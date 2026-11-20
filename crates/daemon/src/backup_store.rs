//! Where backup files are kept, addressed by keys such as `<service id>/<file name>`.
//!
//! Each backup names its store with `storage` and `storage_id`: the data directory, or an
//! S3-compatible storage connection. Both stream: uploads go out in 8 MiB multipart chunks (at
//! most [`S3_UPLOAD_CONCURRENCY`] in flight) and downloads are read as the body arrives, so a
//! backup is never held in memory whole.

use crate::secrets::Secrets;
use anyhow::{Context as _, Result, anyhow, bail};
use common::{SecretValue, StorageConnection};
use futures_util::TryStreamExt as _;
use s3::Bucket;
use s3::creds::Credentials;
use s3::error::S3Error;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tokio::io::AsyncRead;

/// Multipart chunks uploaded in parallel; each holds 8 MiB in memory.
const S3_UPLOAD_CONCURRENCY: usize = 2;
/// Longest a single request (one part, or a whole download) may take.
const S3_TRANSFER_TIMEOUT: Duration = Duration::from_secs(6 * 3600);

pub enum BackupStore {
    /// Files under a directory of the daemon's data dir.
    Local { root: PathBuf },
    /// Objects under `prefix` in an S3 bucket.
    S3 { bucket: Box<Bucket>, prefix: String },
}

/// One file in a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub key: String,
    pub size_bytes: u64,
}

impl BackupStore {
    /// An S3 store for a storage connection, signing with the given credentials.
    pub fn s3(
        conn: &StorageConnection,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<Self> {
        let region = s3::Region::Custom {
            region: conn.region.clone(),
            endpoint: conn.endpoint.trim_end_matches('/').to_string(),
        };
        let credentials = Credentials::new(
            Some(access_key_id),
            Some(secret_access_key),
            None,
            None,
            None,
        )
        .map_err(|e| anyhow!("invalid S3 credentials: {e}"))?;
        let mut bucket = Bucket::new(&conn.bucket, region, credentials).map_err(s3_error)?;
        if conn.path_style {
            bucket = bucket.with_path_style();
        }
        bucket.set_request_timeout(Some(S3_TRANSFER_TIMEOUT));
        Ok(Self::S3 {
            bucket,
            prefix: conn.path_prefix.trim_matches('/').to_string(),
        })
    }

    /// The S3 store of a connection, with its credentials secret decrypted.
    pub async fn for_connection(conn: &StorageConnection, secrets: &Secrets) -> Result<Self> {
        match secrets.value(&conn.credentials_secret_id).await? {
            SecretValue::S3Credentials {
                access_key_id,
                secret_access_key,
            } => Self::s3(conn, &access_key_id, &secret_access_key),
            other => bail!(
                "the credentials of storage '{}' are {}, not s3 credentials",
                conn.name,
                other.kind().label()
            ),
        }
    }

    /// Same store with a shorter per-request timeout, for quick probes.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        if let Self::S3 { bucket, .. } = &mut self {
            bucket.set_request_timeout(Some(timeout));
        }
        self
    }

    /// Moves a finished local file into the store under `key` and returns its size.
    pub async fn put(&self, key: &str, file: &Path) -> Result<u64> {
        match self {
            Self::S3 { bucket, prefix } => {
                let object = object_key(prefix, key)?;
                let mut reader = tokio::fs::File::open(file)
                    .await
                    .with_context(|| format!("cannot open {}", file.display()))?;
                let uploaded = bucket
                    .put_object_stream_builder(&object)
                    .with_content_type("application/octet-stream")
                    .with_max_concurrent_chunks(S3_UPLOAD_CONCURRENCY)
                    .execute_stream(&mut reader)
                    .await
                    .map_err(s3_error)
                    .with_context(|| format!("cannot upload {object}"))?
                    .uploaded_bytes();
                tokio::fs::remove_file(file)
                    .await
                    .with_context(|| format!("cannot remove {}", file.display()))?;
                Ok(u64::try_from(uploaded).unwrap_or(u64::MAX))
            }
            Self::Local { root } => {
                let path = local_path(root, key)?;
                if let Some(dir) = path.parent() {
                    tokio::fs::create_dir_all(dir)
                        .await
                        .with_context(|| format!("cannot create {}", dir.display()))?;
                }
                tokio::fs::rename(file, &path)
                    .await
                    .with_context(|| format!("cannot move the backup to {}", path.display()))?;
                Ok(tokio::fs::metadata(&path).await?.len())
            }
        }
    }

    /// Opens the file stored under `key` for reading.
    pub async fn get(&self, key: &str) -> Result<Pin<Box<dyn AsyncRead + Send>>> {
        match self {
            Self::S3 { bucket, prefix } => {
                let object = object_key(prefix, key)?;
                let response = bucket
                    .get_object_stream(&object)
                    .await
                    .map_err(s3_error)
                    .with_context(|| format!("cannot download {object}"))?;
                let body = response.bytes.map_err(std::io::Error::other);
                Ok(Box::pin(tokio_util::io::StreamReader::new(body)))
            }
            Self::Local { root } => {
                let path = local_path(root, key)?;
                let file = tokio::fs::File::open(&path)
                    .await
                    .with_context(|| format!("cannot open {}", path.display()))?;
                Ok(Box::pin(file))
            }
        }
    }

    /// Files whose key starts with `<prefix>/`.
    pub async fn list(&self, prefix: &str) -> Result<Vec<StoredFile>> {
        match self {
            Self::S3 {
                bucket,
                prefix: root,
            } => {
                let dir = format!("{}/", object_key(root, prefix)?);
                let strip = object_key(root, "")?;
                let pages = bucket
                    .list(dir.clone(), Some("/".into()))
                    .await
                    .map_err(s3_error)
                    .with_context(|| format!("cannot list {dir}"))?;
                Ok(pages
                    .into_iter()
                    .flat_map(|page| page.contents)
                    .map(|object| StoredFile {
                        key: object.key[strip.len()..].to_string(),
                        size_bytes: object.size,
                    })
                    .collect())
            }
            Self::Local { root } => {
                let dir = local_path(root, prefix)?;
                let mut entries = match tokio::fs::read_dir(&dir).await {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                    Err(e) => {
                        return Err(e).with_context(|| format!("cannot list {}", dir.display()));
                    }
                };
                let mut files = Vec::new();
                while let Some(entry) = entries.next_entry().await? {
                    let meta = entry.metadata().await?;
                    if meta.is_file() {
                        files.push(StoredFile {
                            key: format!("{prefix}/{}", entry.file_name().to_string_lossy()),
                            size_bytes: meta.len(),
                        });
                    }
                }
                Ok(files)
            }
        }
    }

    /// Deletes the file under `key`; a missing file is not an error.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self {
            Self::S3 { bucket, prefix } => {
                let object = object_key(prefix, key)?;
                match bucket.delete_object(&object).await {
                    Ok(_) | Err(S3Error::HttpFailWithBody(404, _)) => Ok(()),
                    Err(e) => Err(s3_error(e)).with_context(|| format!("cannot delete {object}")),
                }
            }
            Self::Local { root } => {
                let path = local_path(root, key)?;
                match tokio::fs::remove_file(&path).await {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e).with_context(|| format!("cannot delete {}", path.display())),
                }
            }
        }
    }
}

/// Key of a backup file: `<service id>/<file name>`.
pub fn backup_key(service_id: &str, file_name: &str) -> String {
    format!("{service_id}/{file_name}")
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'))
}

/// Maps a key to a path under `root`, refusing keys that could escape it.
fn local_path(root: &Path, key: &str) -> Result<PathBuf> {
    if !valid_key(key) {
        bail!("invalid storage key '{key}'");
    }
    Ok(root.join(key))
}

/// The object key of `key` under a connection's path prefix. An empty `key` gives the prefix
/// itself, with its trailing slash.
fn object_key(prefix: &str, key: &str) -> Result<String> {
    if !key.is_empty() && !valid_key(key) {
        bail!("invalid storage key '{key}'");
    }
    Ok(match (prefix.is_empty(), key.is_empty()) {
        (true, _) => key.to_string(),
        (false, true) => format!("{prefix}/"),
        (false, false) => format!("{prefix}/{key}"),
    })
}

/// A readable error: the S3 error code and message instead of the raw XML body.
pub fn s3_error(e: S3Error) -> anyhow::Error {
    let S3Error::HttpFailWithBody(status, body) = &e else {
        return anyhow!(e);
    };
    let tag = |name: &str| {
        let start = body.find(&format!("<{name}>"))? + name.len() + 2;
        let end = body[start..].find(&format!("</{name}>"))? + start;
        Some(body[start..end].to_string())
    };
    match (tag("Code"), tag("Message")) {
        (Some(code), Some(message)) => anyhow!("S3 returned {status} {code}: {message}"),
        (Some(code), None) => anyhow!("S3 returned {status} {code}"),
        _ => anyhow!("S3 returned {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_store_puts_lists_reads_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = BackupStore::Local {
            root: dir.path().join("backups"),
        };
        let staged = dir.path().join("staged.gz");
        std::fs::write(&staged, b"hello").unwrap();
        let key = backup_key("svc", "1.sql.gz");
        assert_eq!(store.put(&key, &staged).await.unwrap(), 5);
        assert!(!staged.exists());

        let listed = store.list("svc").await.unwrap();
        assert_eq!(
            listed,
            vec![StoredFile {
                key: key.clone(),
                size_bytes: 5
            }]
        );
        let mut body = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut store.get(&key).await.unwrap(), &mut body)
            .await
            .unwrap();
        assert_eq!(body, b"hello");

        store.delete(&key).await.unwrap();
        store.delete(&key).await.unwrap();
        assert!(store.list("svc").await.unwrap().is_empty());
        assert!(store.list("missing").await.unwrap().is_empty());
    }

    /// An S3 store on a fresh MinIO bucket, or `None` when Docker is unavailable.
    async fn minio_store(prefix: &str) -> Option<(BackupStore, Box<Bucket>)> {
        let (endpoint, bucket) = crate::testminio::bucket().await?;
        let conn = StorageConnection {
            id: "st".into(),
            name: "minio".into(),
            endpoint: endpoint.clone(),
            region: crate::testminio::REGION.into(),
            bucket: bucket.clone(),
            path_prefix: prefix.into(),
            path_style: true,
            credentials_secret_id: "sec".into(),
            created_at: chrono::Utc::now(),
            last_test: None,
        };
        let store = BackupStore::s3(
            &conn,
            crate::testminio::ACCESS_KEY,
            crate::testminio::SECRET_KEY,
        )
        .unwrap();
        Some((store, crate::testminio::client(&endpoint, &bucket)))
    }

    async fn read_all(store: &BackupStore, key: &str) -> Vec<u8> {
        let mut body = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut store.get(key).await.unwrap(), &mut body)
            .await
            .unwrap();
        body
    }

    #[tokio::test]
    async fn s3_store_puts_lists_reads_and_deletes_under_its_prefix() {
        let Some((store, raw)) = minio_store("/team/backups/").await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("staged.gz");
        std::fs::write(&staged, b"hello").unwrap();
        let key = backup_key("svc", "1.sql.gz");
        assert_eq!(store.put(&key, &staged).await.unwrap(), 5);
        assert!(!staged.exists(), "put moves the staged file");

        let objects: Vec<String> = raw
            .list("team/".into(), None)
            .await
            .unwrap()
            .into_iter()
            .flat_map(|p| p.contents)
            .map(|o| o.key)
            .collect();
        assert_eq!(objects, vec!["team/backups/svc/1.sql.gz".to_string()]);
        assert_eq!(
            store.list("svc").await.unwrap(),
            vec![StoredFile {
                key: key.clone(),
                size_bytes: 5
            }]
        );
        assert!(store.list("other").await.unwrap().is_empty());
        assert_eq!(read_all(&store, &key).await, b"hello");

        store.delete(&key).await.unwrap();
        store.delete(&key).await.unwrap();
        assert!(store.list("svc").await.unwrap().is_empty());
        assert!(store.get(&key).await.is_err());
        assert!(store.get("../escape").await.is_err());
    }

    #[tokio::test]
    async fn s3_store_uploads_large_files_in_parts_and_streams_them_back() {
        let Some((store, raw)) = minio_store("").await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("big.gz");
        let size = s3::bucket::CHUNK_SIZE * 2 + 12_345;
        let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(&staged, &data).unwrap();
        let key = backup_key("svc", "big.dump.gz");
        assert_eq!(store.put(&key, &staged).await.unwrap(), size as u64);
        let head = raw.head_object(&key).await.unwrap().0;
        let etag = head.e_tag.unwrap_or_default();
        assert!(
            etag.trim_matches('"').ends_with("-3"),
            "a 3-part multipart upload, got etag {etag}"
        );
        assert!(read_all(&store, &key).await == data, "round trip differs");
        store.delete(&key).await.unwrap();
    }

    #[test]
    fn keys_cannot_escape_the_root() {
        let root = Path::new("/data/backups");
        assert!(local_path(root, "svc/a.gz").is_ok());
        assert!(local_path(root, "../etc/passwd").is_err());
        assert!(local_path(root, "/etc/passwd").is_err());
        assert!(local_path(root, "svc//a.gz").is_err());
        assert!(local_path(root, "").is_err());
    }
}
