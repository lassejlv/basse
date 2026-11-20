//! Every product-specific name lives here, so renaming the product is a change to this file
//! plus the `[[bin]]` names in `crates/cli/Cargo.toml` and `crates/daemon/Cargo.toml`.
//! Nothing else in the workspace may hard-code the product name.

/// Display name shown in the TUI and help text.
pub const NAME: &str = "Basse";
/// Name of the CLI/TUI binary, used in hints such as "run `<cli> login`".
pub const CLI_BIN: &str = "basse";
/// Name of the daemon binary.
pub const DAEMON_BIN: &str = "bassed";
/// Prefix for Docker containers, networks, volumes, labels and image tags.
pub const PREFIX: &str = "basse";
/// Prefix for environment variables, e.g. `BASSE_URL`.
pub const ENV_PREFIX: &str = "BASSE";
/// Directory name under `~/.config` and the daemon's default `/var/lib/<dir>`.
pub const DIR: &str = "basse";
/// Docker network shared by the proxy and every service that has a domain.
pub const PROXY_NETWORK: &str = "basse";
/// Container name of the reverse proxy on each server.
pub const PROXY_CONTAINER: &str = "basse-proxy";
/// GitHub repository that publishes releases, as `owner/name`.
pub const REPO: &str = "lassejlv/basse";

/// Full environment variable name for a suffix: `env_var("URL")` is `BASSE_URL`.
pub fn env_var(suffix: &str) -> String {
    format!("{ENV_PREFIX}_{suffix}")
}

/// Docker label key under the product prefix: `label("service")` is `basse.service`.
pub fn label(key: &str) -> String {
    format!("{PREFIX}.{key}")
}
