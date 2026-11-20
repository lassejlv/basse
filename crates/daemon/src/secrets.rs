//! Secrets encrypted at rest with the daemon's master key, and SSH key handling.
//!
//! The master key is 32 random bytes in `<data dir>/master.key` (mode 0600). Each value is
//! sealed with XChaCha20-Poly1305 under a fresh random 24-byte nonce, with the secret's id as
//! associated data, so a ciphertext copied onto another row fails to open. Stored blobs are
//! `version (1 byte) || nonce (24 bytes) || ciphertext`. Plaintext lives only in memory, in
//! buffers that are zeroed when dropped.

use crate::config::set_mode;
use crate::db::Db;
use anyhow::{Context as _, Result, bail};
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use common::{Id, SecretValue};
use rand::RngCore as _;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

const MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const FORMAT_V1: u8 = 1;

/// The key every secret is sealed with.
pub struct MasterKey(Zeroizing<[u8; MASTER_KEY_LEN]>);

impl MasterKey {
    /// Reads the key, creating it with owner-only permissions when the file does not exist.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            let mut bytes = Zeroizing::new([0u8; MASTER_KEY_LEN]);
            rand::rng().fill_bytes(bytes.as_mut());
            write_private(path, bytes.as_ref())?;
            tracing::info!("created the master key at {}", path.display());
        }
        Self::load(path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = Zeroizing::new(
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?,
        );
        let Ok(key) = <[u8; MASTER_KEY_LEN]>::try_from(bytes.as_slice()) else {
            bail!(
                "{} must hold exactly {MASTER_KEY_LEN} bytes; it has {}",
                path.display(),
                bytes.len()
            );
        };
        set_mode(path, 0o600)?;
        Ok(Self(Zeroizing::new(key)))
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(self.0.as_ref().into())
    }

    /// Seals `plaintext`, binding it to `aad` (the owning row's id).
    pub fn seal(&self, aad: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let sealed = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + sealed.len());
        out.push(FORMAT_V1);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Opens a blob from [`Self::seal`]. Fails when the key, the blob or `aad` differ.
    pub fn open(&self, aad: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let Some((&FORMAT_V1, rest)) = blob.split_first() else {
            bail!("unknown secret format");
        };
        if rest.len() < NONCE_LEN {
            bail!("truncated secret");
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        self.cipher()
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: aad.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| {
                anyhow::anyhow!(
                    "cannot decrypt secret {aad}: the master key does not match the one it was stored with"
                )
            })
    }
}

/// Writes a new file with mode 0600 from the start, so its contents are never readable by
/// others, not even briefly.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Sealing and opening of secret rows, shared by the API and the SSH tunnels.
#[derive(Clone)]
pub struct Secrets {
    db: Db,
    key: Arc<MasterKey>,
}

impl Secrets {
    pub fn new(db: Db, key: MasterKey) -> Self {
        Self {
            db,
            key: Arc::new(key),
        }
    }

    pub fn seal(&self, id: &str, value: &SecretValue) -> Result<Vec<u8>> {
        let json = Zeroizing::new(serde_json::to_vec(value)?);
        self.key.seal(id, &json)
    }

    /// Decrypts the value of secret `id`.
    pub async fn value(&self, id: &Id) -> Result<SecretValue> {
        let blob = self
            .db
            .secret_ciphertext(id)
            .await?
            .with_context(|| format!("secret '{id}' not found"))?;
        let json = self.key.open(id, &blob)?;
        serde_json::from_slice(&json).context("the decrypted secret is not valid")
    }

    /// The private key of an `ssh_key` secret.
    pub async fn ssh_private_key(&self, id: &Id) -> Result<Zeroizing<String>> {
        match self.value(id).await? {
            SecretValue::SshKey { private_key } => Ok(Zeroizing::new(private_key)),
            other => bail!("secret '{id}' is {}, not an ssh key", other.kind().label()),
        }
    }
}

/// Checks a value before it is stored and returns its public part.
pub fn validate(value: &SecretValue) -> std::result::Result<Option<String>, String> {
    match value {
        SecretValue::SshKey { private_key } => public_key(private_key).map(Some),
        SecretValue::S3Credentials {
            access_key_id,
            secret_access_key,
        } => {
            let id = access_key_id.trim();
            if id.is_empty() || id.chars().any(char::is_whitespace) {
                return Err("the access key id must be a single word".into());
            }
            if secret_access_key.trim().is_empty() {
                return Err("the secret access key must not be empty".into());
            }
            Ok(Some(id.to_string()))
        }
        SecretValue::Generic { value } => {
            if value.is_empty() {
                return Err("the value must not be empty".into());
            }
            Ok(None)
        }
    }
}

/// Trims what a pasted value picks up around it, so keys copied from a terminal still work.
pub fn normalize(value: SecretValue) -> SecretValue {
    match value {
        SecretValue::SshKey { private_key } => SecretValue::SshKey {
            private_key: format!("{}\n", private_key.trim()),
        },
        SecretValue::S3Credentials {
            access_key_id,
            secret_access_key,
        } => SecretValue::S3Credentials {
            access_key_id: access_key_id.trim().to_string(),
            secret_access_key: secret_access_key.trim().to_string(),
        },
        generic @ SecretValue::Generic { .. } => generic,
    }
}

/// The OpenSSH public key of an unencrypted OpenSSH private key.
pub fn public_key(private_key: &str) -> std::result::Result<String, String> {
    let key = ssh_key::PrivateKey::from_openssh(private_key.trim()).map_err(|_| {
        "not an OpenSSH private key: expected a '-----BEGIN OPENSSH PRIVATE KEY-----' block \
         (convert older PEM keys with `ssh-keygen -p -f <file>`)"
            .to_string()
    })?;
    if key.is_encrypted() {
        return Err(
            "the key is protected by a passphrase; remove it with `ssh-keygen -p -f <file>`".into(),
        );
    }
    key.public_key()
        .to_openssh()
        .map_err(|e| format!("cannot encode the public key: {e}"))
}

/// A new ed25519 key pair as an OpenSSH private key.
pub fn generate_ssh_key(comment: &str) -> Result<Zeroizing<String>> {
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(seed.as_mut());
    let keypair = ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let key = ssh_key::PrivateKey::new(ssh_key::private::KeypairData::Ed25519(keypair), comment)
        .context("cannot build the key")?;
    let pem = key
        .to_openssh(ssh_key::LineEnding::LF)
        .context("cannot encode the key")?;
    Ok(Zeroizing::new(pem.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_values_open_only_with_the_same_key_and_id() {
        let key = MasterKey::from_bytes([7; 32]);
        let blob = key.seal("abc", b"hunter2").unwrap();
        assert!(!blob.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(key.open("abc", &blob).unwrap().as_slice(), b"hunter2");
        assert!(key.open("other", &blob).is_err());
        assert!(MasterKey::from_bytes([8; 32]).open("abc", &blob).is_err());
        let again = key.seal("abc", b"hunter2").unwrap();
        assert_ne!(blob, again, "every seal uses a fresh nonce");
        let mut tampered = blob.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(key.open("abc", &tampered).is_err());
    }

    #[test]
    fn master_key_is_created_once_with_owner_only_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.key");
        let first = MasterKey::load_or_create(&path).unwrap();
        let second = MasterKey::load_or_create(&path).unwrap();
        assert_eq!(first.0.as_ref(), second.0.as_ref());
        assert_eq!(std::fs::read(&path).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(&path, b"short").unwrap();
        assert!(MasterKey::load(&path).is_err());
    }

    #[test]
    fn generated_keys_derive_their_public_key() {
        let private = generate_ssh_key("test@host").unwrap();
        let public = public_key(&private).unwrap();
        assert!(public.starts_with("ssh-ed25519 "), "{public}");
        assert!(public.ends_with(" test@host"), "{public}");
        assert!(public_key("not a key").is_err());
    }

    #[test]
    fn values_are_validated_without_echoing_them() {
        let err = validate(&SecretValue::SshKey {
            private_key: "s3cr3t-material".into(),
        })
        .unwrap_err();
        assert!(!err.contains("s3cr3t"), "{err}");
        assert_eq!(
            validate(&SecretValue::S3Credentials {
                access_key_id: " AKIA1 ".into(),
                secret_access_key: "x".into(),
            })
            .unwrap(),
            Some("AKIA1".into())
        );
        assert!(
            validate(&SecretValue::Generic {
                value: String::new()
            })
            .is_err()
        );
        assert_eq!(
            format!(
                "{:?}",
                SecretValue::Generic {
                    value: "hunter2".into()
                }
            ),
            "SecretValue(generic, redacted)"
        );
    }
}
