use anyhow::{Context as _, Result, bail};
use common::brand;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A daemon the CLI can talk to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub url: String,
    pub token: String,
}

/// `~/.config/<brand dir>/config.toml`: named contexts and the one in use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub current: Option<String>,
    #[serde(default)]
    pub contexts: BTreeMap<String, Context>,
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        if let Some(p) = std::env::var_os(brand::env_var("CONFIG")) {
            return Ok(PathBuf::from(p));
        }
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config")
            }
        };
        Ok(base.join(brand::DIR).join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                toml::from_str(&s).with_context(|| format!("invalid config at {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// The context to use: the `<PREFIX>_URL` + `<PREFIX>_TOKEN` env vars win, then an explicit name, then `current`.
    pub fn resolve(&self, name: Option<&str>) -> Result<(String, Context)> {
        if let (Ok(url), Ok(token)) = (
            std::env::var(brand::env_var("URL")),
            std::env::var(brand::env_var("TOKEN")),
        ) {
            return Ok(("env".into(), Context { url, token }));
        }
        let Some(name) = name.or(self.current.as_deref()) else {
            bail!(
                "no server connected yet: run `{} login <url> --token <token>`",
                brand::CLI_BIN
            );
        };
        match self.contexts.get(name) {
            Some(ctx) => Ok((name.to_string(), ctx.clone())),
            None => bail!(
                "unknown context '{name}': see `{} context list`",
                brand::CLI_BIN
            ),
        }
    }
}
