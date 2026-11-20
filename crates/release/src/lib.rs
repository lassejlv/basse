//! Published releases: the latest tag, verified archive downloads for a target, and atomic
//! replacement of installed binaries. Used by the daemon's self-update and the CLI's `upgrade`.

use anyhow::{Context as _, Result, anyhow, bail};
use common::brand;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::time::Duration;

/// Release target this binary was built for, as used in the archive names the release workflow
/// publishes. `None` on platforms without a published build.
pub fn current_target() -> Option<&'static str> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "other"
    };
    target_for(os, arch)
}

/// Linux builds are static musl binaries, so one archive serves every distribution.
pub fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    })
}

pub fn archive_name(target: &str) -> String {
    format!("{}-{target}.tar.gz", brand::CLI_BIN)
}

/// Major, minor and patch of `v1.2.3`, `1.2.3` or `1.2.3-rc.1` (the suffix is ignored).
pub fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Whether `candidate` is a strictly newer version than `current`; unparsable versions never are.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// Checks `data` against a `sha256sum`-style file: the hex digest, optionally followed by a name.
pub fn verify_sha256(data: &[u8], checksum_file: &str) -> Result<()> {
    let expected = checksum_file
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("the checksum file is not a sha256 digest");
    }
    let actual = hex::encode(Sha256::digest(data));
    if actual != expected {
        bail!("checksum mismatch: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Unpacks a release archive into `dir`.
pub fn unpack(archive: &[u8], dir: &Path) -> Result<()> {
    tar::Archive::new(flate2::read::GzDecoder::new(archive))
        .unpack(dir)
        .context("cannot unpack the release archive")
}

/// Replaces `dest` with `src` atomically: the copy lands next to `dest` and is renamed over it,
/// so a running binary keeps its old inode and nothing ever sees a half-written file.
pub fn install(src: &Path, dest: &Path) -> Result<()> {
    let name = dest
        .file_name()
        .ok_or_else(|| anyhow!("{} is not a file path", dest.display()))?;
    let tmp = dest.with_file_name(format!(".{}.new", name.to_string_lossy()));
    std::fs::copy(src, &tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, dest).with_context(|| format!("cannot replace {}", dest.display()))
}

/// The version a binary reports with `--version`, e.g. `0.2.0`.
pub fn binary_version(path: &Path) -> Result<String> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("cannot run {}", path.display()))?;
    if !out.status.success() {
        bail!("{} --version failed with {}", path.display(), out.status);
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{} printed no version", path.display()))
}

/// Where releases come from: GitHub, or the directory named by `<PREFIX>_RELEASE_URL`, which
/// mirrors GitHub's layout (`<url>/latest` answers like the releases API and assets live under
/// `<url>/download/<tag>/`). The override exists for testing updates against a local build.
pub struct Releases {
    http: reqwest::Client,
    latest_url: String,
    download_url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

impl Releases {
    pub fn new() -> Result<Self> {
        let (latest_url, download_url) = match std::env::var(brand::env_var("RELEASE_URL")) {
            Ok(base) if !base.trim().is_empty() => {
                let base = base.trim().trim_end_matches('/');
                (format!("{base}/latest"), format!("{base}/download"))
            }
            _ => (
                format!(
                    "https://api.github.com/repos/{}/releases/latest",
                    brand::REPO
                ),
                format!("https://github.com/{}/releases/download", brand::REPO),
            ),
        };
        let http = reqwest::Client::builder()
            .user_agent(format!("{}/{}", brand::CLI_BIN, env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self {
            http,
            latest_url,
            download_url,
        })
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response> {
        let res = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        if !res.status().is_success() {
            bail!("{url} returned {}", res.status());
        }
        Ok(res)
    }

    /// Tag of the latest published release, e.g. `v0.2.0`.
    pub async fn latest_tag(&self) -> Result<String> {
        let release: LatestRelease = self
            .get(&self.latest_url)
            .await?
            .json()
            .await
            .context("unexpected answer from the releases API")?;
        Ok(release.tag_name)
    }

    /// Downloads the archive of `tag` for `target` and verifies it against its `.sha256`.
    pub async fn download(&self, tag: &str, target: &str) -> Result<Vec<u8>> {
        let name = archive_name(target);
        let url = format!("{}/{tag}/{name}", self.download_url);
        let archive = self.get(&url).await?.bytes().await?;
        let checksum = self.get(&format!("{url}.sha256")).await?.text().await?;
        verify_sha256(&archive, &checksum)
            .with_context(|| format!("{name} failed verification"))?;
        Ok(archive.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "v0.2.0"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("v0.3.0-rc.1", "0.3.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn targets_match_published_archives() {
        assert_eq!(
            target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(target_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(target_for("windows", "x86_64"), None);
        assert_eq!(
            archive_name("x86_64-apple-darwin"),
            format!("{}-x86_64-apple-darwin.tar.gz", brand::CLI_BIN)
        );
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(current_target(), Some("x86_64-unknown-linux-musl"));
        }
    }

    #[test]
    fn checksums_are_verified() {
        let data = b"release bytes";
        let digest = hex::encode(Sha256::digest(data));
        verify_sha256(data, &format!("{digest}  archive.tar.gz\n")).unwrap();
        verify_sha256(data, &digest.to_uppercase()).unwrap();
        let err = verify_sha256(b"tampered", &digest).unwrap_err();
        assert!(err.to_string().contains("mismatch"));
        assert!(verify_sha256(data, "").is_err());
        assert!(verify_sha256(data, "not-a-digest  file").is_err());
    }

    #[test]
    fn install_replaces_the_destination() {
        let dir = std::env::temp_dir().join(format!("release-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dest) = (dir.join("new"), dir.join("bin"));
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&dest, "old").unwrap();
        install(&src, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
        assert!(!dir.join(".bin.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
