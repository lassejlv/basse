//! The pages the proxy serves when it cannot answer for an app itself: an unknown host, a
//! service with no container to forward to, one that is still starting, a timeout and any other
//! error the proxy raises. Errors an app returns itself pass through untouched.
//!
//! The pages are Caddy templates that the daemon writes into the proxy container next to a base
//! Caddyfile, so they keep working while the daemon is down. The Caddyfile holds the error
//! handling every route imports, a catch-all for hosts nothing serves, and a route for each
//! domain whose service has no container to answer: stopped, never deployed, or deploying for the
//! first time. Those routes keep the domain's certificate managed while no container carries
//! its labels.

use anyhow::Result;
use base64::Engine as _;
use common::brand;
use std::fmt::Write as _;

const TEMPLATE: &str = include_str!("../pages/page.html");
const FONT: &[u8] = include_bytes!("../pages/jetbrains-mono.woff2");
const ART: &[u8] = include_bytes!("../pages/lighthouse.png");

/// Bump when the proxy container must be recreated to pick up how the pages are wired, so
/// [`crate::api::sync_proxy`] replaces proxies created by an older daemon.
pub const VERSION: &str = "1";
/// Seconds before a starting page reloads itself, also sent as `Retry-After`.
const STARTING_RETRY: u32 = 10;
/// `Retry-After` while no server is available.
const DOWN_RETRY: u32 = 30;

/// Name of the Caddyfile snippet holding the error handling every route imports.
pub fn snippet() -> String {
    format!("{}_errors", brand::PREFIX)
}

/// Directory in the proxy container holding the base Caddyfile and the pages.
pub fn dir() -> String {
    format!("/srv/{}", brand::PREFIX)
}

pub fn caddyfile_path() -> String {
    format!("{}/Caddyfile", dir())
}

/// Why a domain gets a fallback route instead of a container's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// A deployment is in progress and no container serves the domain yet.
    Starting,
    /// No container serves the domain: the service is stopped or not deployed.
    Down,
}

/// A domain whose service has no container to answer for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldHost {
    pub host: String,
    pub tls: bool,
    pub hold: Hold,
}

struct Page {
    file: &'static str,
    code: &'static str,
    label: &'static str,
    dot: &'static str,
    headline: &'static str,
    /// HTML; `{{html .Host}}` is the requested host.
    line: &'static str,
    /// The `error` of the JSON body other clients get; `{host}` is the requested host.
    json: &'static str,
    reload: bool,
}

const NOTHING_HERE: Page = Page {
    file: "nothing-here",
    code: "404",
    label: "nothing here",
    dot: "muted",
    headline: "Nothing here.",
    line: "No app is set up for {{html .Host}} on this server.",
    json: "no app is set up for {host} on this server",
    reload: false,
};

const STARTING: Page = Page {
    file: "starting",
    code: "503",
    label: "starting",
    dot: "info",
    headline: "Starting up.",
    line: "{{html .Host}} is deploying. This page reloads every 10 s.",
    json: "{host} is starting up, try again in a few seconds",
    reload: true,
};

const NO_SERVER: Page = Page {
    file: "no-server",
    code: "503",
    label: "no server",
    dot: "err",
    headline: "No server available.",
    line: "{{html .Host}} has no running container right now.",
    json: "no server is available for {host} right now",
    reload: true,
};

const TIMED_OUT: Page = Page {
    file: "timed-out",
    code: "504",
    label: "timeout",
    dot: "err",
    headline: "Timed out.",
    line: "{{html .Host}} took too long to answer.",
    json: "{host} took too long to answer",
    reload: true,
};

const WENT_WRONG: Page = Page {
    file: "went-wrong",
    code: "{{placeholder \"http.error.status_code\"}}",
    label: "error",
    dot: "err",
    headline: "Something went wrong.",
    line: "The proxy couldn't finish this request.",
    json: "the proxy could not finish this request",
    reload: true,
};

const PAGES: [&Page; 5] = [
    &NOTHING_HERE,
    &STARTING,
    &NO_SERVER,
    &TIMED_OUT,
    &WENT_WRONG,
];

const RELOAD: &str = r#"
<a class="key" href="" aria-keyshortcuts="r"><span>r</span>reload</a>
<script>addEventListener("keydown",e=>{if(e.key==="r"&&!(e.ctrlKey||e.metaKey||e.altKey))location.reload()})</script>"#;

fn render(page: &Page) -> String {
    let b64 = |bytes| base64::engine::general_purpose::STANDARD.encode(bytes);
    let starting = page.file == STARTING.file;
    TEMPLATE
        .replace(
            "%%REFRESH%%",
            &if starting {
                format!("\n<meta http-equiv=\"refresh\" content=\"{STARTING_RETRY}\">")
            } else {
                String::new()
            },
        )
        .replace(
            "%%TITLE%%",
            &format!("{} · {}", page.code, page.headline.trim_end_matches('.')),
        )
        .replace("%%FONT%%", &b64(FONT))
        .replace("%%ART%%", &b64(ART))
        .replace("%%BRAND%%", &brand::NAME.to_lowercase())
        .replace("%%HINT%%", if page.reload { RELOAD } else { "" })
        .replace("%%DOT_CLASS%%", if starting { " spin" } else { "" })
        .replace("%%GLYPH%%", if starting { "" } else { "●" })
        .replace("%%DOT%%", page.dot)
        .replace("%%STATUS%%", &format!("{} · {}", page.code, page.label))
        .replace("%%HEADLINE%%", page.headline)
        .replace("%%LINE%%", page.line)
}

/// One branch of the error handling: the page for browsers, a JSON body for everyone else.
fn handle(out: &mut String, matcher: &str, page: &Page, status: &str, retry: Option<u32>) {
    let _ = writeln!(
        out,
        "\t\thandle {matcher}{}{{",
        if matcher.is_empty() { "" } else { " " }
    );
    if let Some(secs) = retry {
        let _ = writeln!(out, "\t\t\theader Retry-After {secs}");
    }
    let _ = writeln!(
        out,
        "\t\t\trespond @json `{{\"error\":\"{}\"}}` {status}",
        page.json
    );
    let _ = writeln!(out, "\t\t\trewrite * /{}.html", page.file);
    let _ = writeln!(
        out,
        "\t\t\tfile_server {{\n\t\t\t\tstatus {status}\n\t\t\t}}"
    );
    let _ = writeln!(out, "\t\t}}");
}

/// The base Caddyfile the proxy extends with the routes of running containers.
pub fn caddyfile(held: &[HeldHost]) -> String {
    let snippet = snippet();
    let mut out = String::new();
    let _ = writeln!(out, "({snippet}) {{\n\thandle_errors {{");
    let _ = writeln!(out, "\t\troot * {}/pages", dir());
    out.push_str(
        "\t\ttemplates\n\t\theader Cache-Control no-store\n\t\t@json not header Accept *text/html*\n\t\theader @json Content-Type application/json\n",
    );
    let starting: Vec<&str> = held
        .iter()
        .filter(|h| h.hold == Hold::Starting)
        .map(|h| h.host.as_str())
        .collect();
    if !starting.is_empty() {
        let _ = writeln!(out, "\t\t@starting host {}", starting.join(" "));
        handle(
            &mut out,
            "@starting",
            &STARTING,
            "503",
            Some(STARTING_RETRY),
        );
    }
    out.push_str("\t\t@missing expression {err.status_code} == 404\n");
    handle(&mut out, "@missing", &NOTHING_HERE, "404", None);
    out.push_str("\t\t@down expression {err.status_code} in [502, 503]\n");
    handle(&mut out, "@down", &NO_SERVER, "503", Some(DOWN_RETRY));
    out.push_str("\t\t@timeout expression {err.status_code} == 504\n");
    handle(&mut out, "@timeout", &TIMED_OUT, "504", None);
    handle(&mut out, "", &WENT_WRONG, "{err.status_code}", None);
    out.push_str("\t}\n}\n");
    for h in held {
        let site = if h.tls {
            h.host.clone()
        } else {
            format!("http://{}", h.host)
        };
        let _ = writeln!(out, "\n{site} {{\n\timport {snippet}\n\terror 503\n}}");
    }
    let _ = writeln!(out, "\nhttp:// {{\n\timport {snippet}\n\terror 404\n}}");
    out
}

/// A tar archive of the base Caddyfile and the pages, to extract at `/srv` in the proxy.
pub fn archive(held: &[HeldHost]) -> Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut add = |path: String, data: &[u8], dir: bool| {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(if dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        header.set_mode(if dir { 0o755 } else { 0o644 });
        header.set_size(data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, path, data)
    };
    add(format!("{}/", brand::PREFIX), &[], true)?;
    add(format!("{}/pages/", brand::PREFIX), &[], true)?;
    add(
        format!("{}/Caddyfile", brand::PREFIX),
        caddyfile(held).as_bytes(),
        false,
    )?;
    for page in PAGES {
        add(
            format!("{}/pages/{}.html", brand::PREFIX, page.file),
            render(page).as_bytes(),
            false,
        )?;
    }
    Ok(tar.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(host: &str, tls: bool, hold: Hold) -> HeldHost {
        HeldHost {
            host: host.into(),
            tls,
            hold,
        }
    }

    #[test]
    fn pages_are_self_contained_and_small() {
        for page in PAGES {
            let html = render(page);
            assert!(!html.contains("%%"), "{}: unreplaced token", page.file);
            assert!(
                html.len() < 30 * 1024,
                "{}: {} bytes",
                page.file,
                html.len()
            );
            assert!(!html.contains("http://") && !html.contains("https://"));
            assert!(html.contains(&brand::NAME.to_lowercase()));
            assert!(html.contains("{{html .Host}}"));
            assert!(html.contains("http.request.uuid"));
        }
        assert!(render(&STARTING).contains("http-equiv=\"refresh\""));
        assert!(!render(&NO_SERVER).contains("http-equiv"));
        assert!(!render(&NOTHING_HERE).contains("reload"));
    }

    #[test]
    fn the_caddyfile_holds_domains_without_a_container() {
        let file = caddyfile(&[
            held("new.example.com", true, Hold::Starting),
            held("off.localhost", false, Hold::Down),
        ]);
        let snippet = snippet();
        assert!(file.starts_with(&format!("({snippet}) {{")));
        assert!(file.contains("@starting host new.example.com\n"));
        assert!(file.contains(&format!(
            "\nnew.example.com {{\n\timport {snippet}\n\terror 503\n}}"
        )));
        assert!(file.contains(&format!(
            "\nhttp://off.localhost {{\n\timport {snippet}\n\terror 503\n}}"
        )));
        assert!(file.contains(&format!(
            "\nhttp:// {{\n\timport {snippet}\n\terror 404\n}}"
        )));
        assert!(!caddyfile(&[]).contains("@starting"));
    }

    #[test]
    fn the_archive_unpacks_under_the_prefix() {
        let bytes = archive(&[]).unwrap();
        let mut names: Vec<String> = tar::Archive::new(bytes.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        names.sort();
        let p = brand::PREFIX;
        assert!(names.contains(&format!("{p}/Caddyfile")));
        assert!(names.contains(&format!("{p}/pages/no-server.html")));
        assert_eq!(names.iter().filter(|n| n.ends_with(".html")).count(), 5);
    }
}
