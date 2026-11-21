//! Self-update: replace the daemon, and the CLI installed beside it, with a published release,
//! then restart into the new binary.

use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use common::api::UpdateRequest;
use common::{ActivityLevel, UpdateResult, brand};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Exit code that tells the supervisor the daemon stopped on purpose to be restarted.
const RESTART_EXIT_CODE: i32 = 75;

/// systemd sets `INVOCATION_ID` for every service it runs; with `Restart=always` it brings the
/// daemon back after it exits.
pub fn supervised() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

/// Installs the requested release, or answers `updated: false` when it is not newer than the
/// running version and `force` is off.
pub async fn run(state: &AppState, req: UpdateRequest) -> ApiResult<UpdateResult> {
    let releases = release::Releases::new()?;
    let tag = releases.resolve(req.version.as_deref()).await?;
    let current = env!("CARGO_PKG_VERSION");
    if !req.force && !release::is_newer(&tag, current) {
        return Ok(UpdateResult {
            from: current.to_string(),
            to: current.to_string(),
            updated: false,
        });
    }
    if !supervised() && !req.force {
        bad_request!(
            "{daemon} is not running under systemd, so nothing would start it again after the update; \
             run it as a service (`{daemon} install-service`) or update with force (`{cli} update --server --force`) to restart it in place",
            daemon = brand::DAEMON_BIN,
            cli = brand::CLI_BIN
        );
    }
    refuse_while_deploying(state).await?;
    let target = release::current_target()
        .ok_or_else(|| AppError::BadRequest("no release is published for this platform".into()))?;
    let exe = std::env::current_exe()?;
    let archive = releases.download(&tag, target).await?;
    let dir = state.dirs.builds_dir().join(format!("update-{tag}"));
    let installed = install(&archive, &dir, &exe);
    let _ = std::fs::remove_dir_all(&dir);
    let to = installed?;
    refuse_while_deploying(state).await?;
    state
        .activity(
            ActivityLevel::Success,
            format!("Updated {} to {to}", brand::DAEMON_BIN),
            Some(format!("from {current}")),
        )
        .await;
    restart_soon(exe);
    Ok(UpdateResult {
        from: current.to_string(),
        to,
        updated: true,
    })
}

async fn refuse_while_deploying(state: &AppState) -> ApiResult<()> {
    let active = state.db.unfinished_deployments().await?;
    if !active.is_empty() {
        let names: Vec<String> = active
            .iter()
            .map(|(service, number)| format!("{service} #{number}"))
            .collect();
        conflict!(
            "deployments in progress: {}; wait for them to finish or cancel them first",
            names.join(", ")
        );
    }
    Ok(())
}

/// Unpacks the archive, checks the new daemon runs, then swaps in the daemon and the CLI.
/// Returns the new version.
fn install(archive: &[u8], dir: &Path, exe: &Path) -> anyhow::Result<String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    release::unpack(archive, dir)?;
    let daemon = dir.join(brand::DAEMON_BIN);
    if !daemon.is_file() {
        anyhow::bail!("the release archive has no {}", brand::DAEMON_BIN);
    }
    let version = release::binary_version(&daemon)?;
    release::install(&daemon, exe)?;
    let cli = exe.with_file_name(brand::CLI_BIN);
    if cli.is_file() && dir.join(brand::CLI_BIN).is_file() {
        release::install(&dir.join(brand::CLI_BIN), &cli)?;
    }
    Ok(version)
}

/// Gives the response time to reach the client, then hands over to the new binary: exit for
/// systemd to restart it, or exec it in place when nothing supervises the daemon.
fn restart_soon(exe: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(750)).await;
        if supervised() {
            tracing::info!("exiting so systemd starts the new version");
            std::process::exit(RESTART_EXIT_CODE);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            tracing::info!("restarting into {}", exe.display());
            let err = std::process::Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .exec();
            tracing::error!("cannot restart into the new version: {err}");
        }
        std::process::exit(RESTART_EXIT_CODE);
    });
}
