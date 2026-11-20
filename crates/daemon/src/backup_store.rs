//! Where backup files are kept, addressed by keys such as `<service id>/<file name>`.
//!
//! Only local storage exists today. Remote stores become new variants with the same four
//! operations, selected per backup by its `storage` and `storage_id`.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::io::AsyncRead;

pub enum BackupStore {
    /// Files under a directory of the daemon's data dir.
    Local { root: PathBuf },
}

/// One file in a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub key: String,
    pub size_bytes: u64,
}

impl BackupStore {
    /// Moves a finished local file into the store under `key` and returns its size.
    pub async fn put(&self, key: &str, file: &Path) -> Result<u64> {
        match self {
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

/// Maps a key to a path under `root`, refusing keys that could escape it.
fn local_path(root: &Path, key: &str) -> Result<PathBuf> {
    let valid = !key.is_empty()
        && key
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'));
    if !valid {
        bail!("invalid storage key '{key}'");
    }
    Ok(root.join(key))
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
