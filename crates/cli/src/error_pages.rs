//! `service error-page` and `proxy error-page`: the pages the proxy serves when it cannot answer
//! for an app itself, per service and as the instance default.

use crate::output::{self, Cell, Tone, muted};
use anyhow::{Context as _, Result, bail};
use clap::{Subcommand, ValueEnum};
use client::Client;
use common::api::{SetErrorPage, UpdateService};
use common::*;
use std::path::{Path, PathBuf};

/// Pages are 404, 503, starting, 504 or 5xx.
#[derive(Subcommand)]
pub enum ServiceErrorPageCmd {
    /// Whether branded pages are on, and the page served for each error.
    List { service: String },
    /// Replace a page with an HTML file from this computer, or with --repo, a file in the
    /// service's repository (relative to its root directory) read on every deploy.
    Set {
        service: String,
        page: ErrorPageKind,
        #[arg(required_unless_present = "repo")]
        file: Option<PathBuf>,
        #[arg(long, conflicts_with = "file")]
        repo: Option<String>,
    },
    /// Serve the instance default or the built-in page again.
    Reset {
        service: String,
        page: ErrorPageKind,
    },
    /// Turn branded pages on or off. Off lets the proxy's own responses through: an empty
    /// 502, 503 or 504.
    Branded { service: String, state: OnOff },
}

/// Defaults for every service that has no page of its own, and for unknown hosts. Pages are
/// 404, 503, starting, 504 or 5xx.
#[derive(Subcommand)]
pub enum InstanceErrorPageCmd {
    /// The page served for each error.
    List,
    /// Replace a page with an HTML file from this computer.
    Set { page: ErrorPageKind, file: PathBuf },
    /// Serve the built-in page again.
    Reset { page: ErrorPageKind },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

pub async fn service(c: &Client, cmd: ServiceErrorPageCmd, json: bool) -> Result<()> {
    match cmd {
        ServiceErrorPageCmd::List { service } => {
            let (_, _, s) = crate::find(c, &service).await?;
            let pages = c.service(&s.id).await?.error_pages;
            if json {
                return output::json(&pages);
            }
            let instance = c.instance_error_pages().await?;
            if pages.branded {
                println!(
                    "{} {}",
                    muted("branded pages"),
                    output::paint("● on", Tone::Ok)
                );
            } else {
                println!(
                    "{} {}",
                    muted("branded pages"),
                    muted("○ off · the proxy's own responses pass through")
                );
            }
            print_pages(&pages, Some(&instance));
        }
        ServiceErrorPageCmd::Set {
            service,
            page,
            file,
            repo,
        } => {
            let (_, _, s) = crate::find(c, &service).await?;
            let body = match (file, repo) {
                (_, Some(path)) => SetErrorPage::Repo { path },
                (Some(file), None) => upload(&file)?,
                (None, None) => bail!("give a file, or --repo with a path in the repository"),
            };
            let set = c.set_error_page(Some(&s.id), page, &body).await?;
            if json {
                return output::json(&set);
            }
            match &set.source {
                ErrorPageSource::Repo { path } => output::ok(format!(
                    "{} reads its {page} page from {path} on every deploy; deploy to apply",
                    s.name
                )),
                ErrorPageSource::Upload { .. } => output::ok(format!(
                    "{} serves your {page} page ({})",
                    s.name,
                    output::size(set.size.unwrap_or_default())
                )),
            }
            warnings(&set);
        }
        ServiceErrorPageCmd::Reset { service, page } => {
            let (_, _, s) = crate::find(c, &service).await?;
            c.delete_error_page(Some(&s.id), page).await?;
            output::ok(format!("{} serves the default {page} page again", s.name));
        }
        ServiceErrorPageCmd::Branded { service, state } => {
            let (_, _, s) = crate::find(c, &service).await?;
            let branded = matches!(state, OnOff::On);
            let d = c
                .update_service(
                    &s.id,
                    &UpdateService {
                        branded_error_pages: Some(branded),
                        ..Default::default()
                    },
                )
                .await?;
            if json {
                return output::json(&d.error_pages);
            }
            if branded {
                output::ok(format!("{} serves branded error pages", s.name));
            } else {
                output::ok(format!(
                    "{} lets the proxy's own error responses through",
                    s.name
                ));
            }
        }
    }
    Ok(())
}

pub async fn instance(c: &Client, cmd: InstanceErrorPageCmd, json: bool) -> Result<()> {
    match cmd {
        InstanceErrorPageCmd::List => {
            let pages = c.instance_error_pages().await?;
            if json {
                return output::json(&pages);
            }
            print_pages(&pages, None);
        }
        InstanceErrorPageCmd::Set { page, file } => {
            let set = c.set_error_page(None, page, &upload(&file)?).await?;
            if json {
                return output::json(&set);
            }
            output::ok(format!(
                "every service without its own {page} page serves yours ({})",
                output::size(set.size.unwrap_or_default())
            ));
            warnings(&set);
        }
        InstanceErrorPageCmd::Reset { page } => {
            c.delete_error_page(None, page).await?;
            output::ok(format!("the built-in {page} page is the default again"));
        }
    }
    Ok(())
}

fn upload(file: &Path) -> Result<SetErrorPage> {
    let size = std::fs::metadata(file)
        .with_context(|| format!("cannot read {}", file.display()))?
        .len();
    if size > MAX_ERROR_PAGE_BYTES as u64 {
        bail!(
            "{} is {}; error pages can be at most {}",
            file.display(),
            output::size(size),
            output::size(MAX_ERROR_PAGE_BYTES as u64)
        );
    }
    let html = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {} as UTF-8 text", file.display()))?;
    Ok(SetErrorPage::Upload {
        html,
        name: file.file_name().map(|n| n.to_string_lossy().into_owned()),
    })
}

fn warnings(page: &CustomErrorPage) {
    for w in &page.warnings {
        output::warn(w);
    }
}

/// One row per page: the service's own, else the instance default, else the built-in page.
fn print_pages(pages: &ErrorPages, instance: Option<&ErrorPages>) {
    let rows = ErrorPageKind::ALL
        .into_iter()
        .map(|kind| {
            let (source, page) = match (pages.page(kind), instance.and_then(|i| i.page(kind))) {
                (Some(p), _) => (source(&p.source), Some(p)),
                (None, Some(p)) if p.size.is_some() => ("instance default".to_string(), Some(p)),
                _ => ("built-in".to_string(), None),
            };
            let size = page
                .and_then(|p| p.size)
                .map_or_else(String::new, output::size);
            let note = page.map(|p| p.warnings.join("; ")).unwrap_or_default();
            vec![
                Cell(kind.code().into(), Tone::Text),
                Cell(kind.label().into(), Tone::Text),
                Cell(
                    source,
                    if page.is_some() {
                        Tone::Text
                    } else {
                        Tone::Muted
                    },
                ),
                Cell(size, Tone::Muted),
                Cell(note, Tone::Accent),
            ]
        })
        .collect();
    output::table(&["CODE", "PAGE", "SOURCE", "SIZE", ""], rows);
}

fn source(source: &ErrorPageSource) -> String {
    match source {
        ErrorPageSource::Upload { name: Some(name) } => format!("uploaded {name}"),
        ErrorPageSource::Upload { name: None } => "uploaded".into(),
        ErrorPageSource::Repo { path } => format!("repo {path}"),
    }
}
