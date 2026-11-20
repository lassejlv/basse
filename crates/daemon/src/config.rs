//! Data directory layout and the daemon's `config.toml`.

use anyhow::{Context as _, Result};
use common::{DEFAULT_PORT, brand};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

/// Default data directory, `/var/lib/<dir>`.
pub fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib").join(brand::DIR)
}

pub fn default_listen() -> String {
    format!("0.0.0.0:{DEFAULT_PORT}")
}

/// Every path the daemon reads or writes, rooted at the data directory.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn db(&self) -> PathBuf {
        self.root.join(format!("{}.db", brand::DIR))
    }

    pub fn ssh_dir(&self) -> PathBuf {
        self.root.join("ssh")
    }

    pub fn ssh_key(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519")
    }

    pub fn ssh_public_key(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519.pub")
    }

    pub fn deployments_dir(&self) -> PathBuf {
        self.root.join("deployments")
    }

    pub fn deployment_log(&self, deployment_id: &str) -> PathBuf {
        self.deployments_dir().join(format!("{deployment_id}.log"))
    }

    pub fn builds_dir(&self) -> PathBuf {
        self.root.join("builds")
    }

    pub fn build_dir(&self, deployment_id: &str) -> PathBuf {
        self.builds_dir().join(deployment_id)
    }

    pub fn is_initialized(&self) -> bool {
        self.config().exists()
    }

    /// Creates the directory tree with owner-only permissions on the root.
    pub fn create(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.ssh_dir(),
            self.deployments_dir(),
            self.builds_dir(),
        ] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        set_mode(&self.root, 0o700)?;
        set_mode(&self.ssh_dir(), 0o700)
    }
}

/// Contents of `config.toml`. Only a hash of the API token is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Hex-encoded SHA-256 of the API token.
    pub token_sha256: String,
    #[serde(default = "default_listen")]
    pub listen: String,
}

impl Config {
    pub fn load(dir: &DataDir) -> Result<Self> {
        Self::load_path(&dir.config())
    }

    pub fn load_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("invalid config at {}", path.display()))
    }

    pub fn save(&self, dir: &DataDir) -> Result<()> {
        let path = dir.config();
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        set_mode(&path, 0o600)
    }

    pub fn token_hash(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.token_sha256).context("token_sha256 is not hex")?;
        bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("token_sha256 must be 32 bytes"))
    }
}

/// A fresh API token: 32 random bytes, hex encoded.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_round_trips_through_config() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        let config = Config {
            token_sha256: hex::encode(hash_token(&token)),
            listen: default_listen(),
        };
        assert_eq!(config.token_hash().unwrap(), hash_token(&token));
    }
}
