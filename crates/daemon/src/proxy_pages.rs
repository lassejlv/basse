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
//!
//! Every route imports the same snippet, and the snippet tells services apart by host: a
//! service with branded pages off gets Caddy's own empty response, and a service's custom page
//! wins over the instance default, which wins over the built-in page. So a change applies as
//! soon as the proxy rereads the file, without touching any container.

use anyhow::Result;
use base64::Engine as _;
use common::{ErrorPageKind, brand};
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

/// A custom page replacing a built-in one: for a service's hosts, or the instance default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPage {
    /// The service it belongs to; `None` for the instance default.
    pub service_id: Option<String>,
    /// The service's domains; empty for the instance default.
    pub hosts: Vec<String>,
    pub kind: ErrorPageKind,
    pub html: String,
}

/// Everything the base Caddyfile and the pages of one proxy are made from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyPages {
    pub held: Vec<HeldHost>,
    /// Domains of services with branded pages off: their errors pass through as Caddy sends
    /// them, an empty body with the status.
    pub raw: Vec<String>,
    pub custom: Vec<CustomPage>,
}

fn builtin(kind: ErrorPageKind) -> &'static Page {
    match kind {
        ErrorPageKind::NotFound => &NOTHING_HERE,
        ErrorPageKind::NoServer => &NO_SERVER,
        ErrorPageKind::Starting => &STARTING,
        ErrorPageKind::Timeout => &TIMED_OUT,
        ErrorPageKind::Error => &WENT_WRONG,
    }
}

impl CustomPage {
    /// Path under the pages directory.
    fn file(&self) -> String {
        format!(
            "custom/{}/{}.html",
            self.service_id.as_deref().unwrap_or("instance"),
            builtin(self.kind).file
        )
    }
}

/// The status a page answers with and the `Retry-After` it sends.
fn status(kind: ErrorPageKind) -> (&'static str, Option<u32>) {
    match kind {
        ErrorPageKind::NotFound => ("404", None),
        ErrorPageKind::NoServer => ("503", Some(DOWN_RETRY)),
        ErrorPageKind::Starting => ("503", Some(STARTING_RETRY)),
        ErrorPageKind::Timeout => ("504", None),
        ErrorPageKind::Error => ("{err.status_code}", None),
    }
}

/// The error statuses a page is for, as a CEL expression. `Starting` matches by host instead.
fn condition(kind: ErrorPageKind) -> Option<&'static str> {
    match kind {
        ErrorPageKind::NotFound => Some("{err.status_code} == 404"),
        ErrorPageKind::NoServer => Some("{err.status_code} in [502, 503]"),
        ErrorPageKind::Timeout => Some("{err.status_code} == 504"),
        ErrorPageKind::Error => Some("!({err.status_code} in [404, 502, 503, 504])"),
        ErrorPageKind::Starting => None,
    }
}

/// One branch of the error handling: the page for browsers, a JSON body for everyone else.
/// Built-in pages are templates; custom pages are served as they are.
fn handle(out: &mut String, matcher: &str, kind: ErrorPageKind, custom: Option<&CustomPage>) {
    let page = builtin(kind);
    let (status, retry) = status(kind);
    let _ = writeln!(
        out,
        "\t\thandle {matcher}{}{{",
        if matcher.is_empty() { "" } else { " " }
    );
    out.push_str("\t\t\theader Cache-Control no-store\n");
    out.push_str("\t\t\theader @json Content-Type application/json\n");
    if let Some(secs) = retry {
        let _ = writeln!(out, "\t\t\theader Retry-After {secs}");
    }
    let _ = writeln!(
        out,
        "\t\t\trespond @json `{{\"error\":\"{}\"}}` {status}",
        page.json
    );
    match custom {
        Some(c) => {
            let _ = writeln!(out, "\t\t\trewrite * /{}", c.file());
        }
        None => {
            out.push_str("\t\t\ttemplates\n");
            let _ = writeln!(out, "\t\t\trewrite * /{}.html", page.file);
        }
    }
    let _ = writeln!(
        out,
        "\t\t\tfile_server {{\n\t\t\t\tstatus {status}\n\t\t\t}}"
    );
    let _ = writeln!(out, "\t\t}}");
}

/// A named matcher on some hosts and, optionally, a CEL expression.
fn matcher(out: &mut String, name: &str, hosts: &[&str], expression: Option<&str>) {
    match expression {
        None => {
            let _ = writeln!(out, "\t\t@{name} host {}", hosts.join(" "));
        }
        Some(expr) => {
            let _ = writeln!(
                out,
                "\t\t@{name} {{\n\t\t\thost {}\n\t\t\texpression `{expr}`\n\t\t}}",
                hosts.join(" ")
            );
        }
    }
}

/// The base Caddyfile the proxy extends with the routes of running containers. The error
/// handling tries, in order: hosts with branded pages off, starting hosts, each service's
/// custom pages, then the instance defaults and built-in pages.
pub fn caddyfile(pages: &ProxyPages) -> String {
    let snippet = snippet();
    let mut out = String::new();
    let _ = writeln!(out, "({snippet}) {{\n\thandle_errors {{");
    let _ = writeln!(out, "\t\troot * {}/pages", dir());
    out.push_str("\t\t@json not header Accept *text/html*\n");
    if !pages.raw.is_empty() {
        let raw: Vec<&str> = pages.raw.iter().map(String::as_str).collect();
        matcher(&mut out, "raw", &raw, None);
        out.push_str("\t\thandle @raw {\n\t\t\trespond \"\" {err.status_code}\n\t\t}\n");
    }
    let starting: Vec<&str> = pages
        .held
        .iter()
        .filter(|h| h.hold == Hold::Starting)
        .map(|h| h.host.as_str())
        .collect();
    let instance = |kind| {
        pages
            .custom
            .iter()
            .find(|c| c.service_id.is_none() && c.kind == kind)
    };
    let services = pages.custom.iter().filter(|c| c.service_id.is_some());
    for (i, c) in services.clone().enumerate() {
        if c.kind != ErrorPageKind::Starting {
            continue;
        }
        let hosts: Vec<&str> = c
            .hosts
            .iter()
            .map(String::as_str)
            .filter(|h| starting.contains(h))
            .collect();
        if !hosts.is_empty() {
            let name = format!("custom_{i}");
            matcher(&mut out, &name, &hosts, None);
            handle(&mut out, &format!("@{name}"), c.kind, Some(c));
        }
    }
    if !starting.is_empty() {
        matcher(&mut out, "starting", &starting, None);
        let kind = ErrorPageKind::Starting;
        handle(&mut out, "@starting", kind, instance(kind));
    }
    for (i, c) in services.enumerate() {
        let Some(expr) = condition(c.kind) else {
            continue;
        };
        if c.hosts.is_empty() {
            continue;
        }
        let name = format!("custom_{i}");
        let hosts: Vec<&str> = c.hosts.iter().map(String::as_str).collect();
        matcher(&mut out, &name, &hosts, Some(expr));
        handle(&mut out, &format!("@{name}"), c.kind, Some(c));
    }
    for (name, kind) in [
        ("missing", ErrorPageKind::NotFound),
        ("down", ErrorPageKind::NoServer),
        ("timeout", ErrorPageKind::Timeout),
    ] {
        let _ = writeln!(
            out,
            "\t\t@{name} expression `{}`",
            condition(kind).unwrap_or_default()
        );
        handle(&mut out, &format!("@{name}"), kind, instance(kind));
    }
    let kind = ErrorPageKind::Error;
    handle(&mut out, "", kind, instance(kind));
    out.push_str("\t}\n}\n");
    for h in &pages.held {
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
pub fn archive(pages: &ProxyPages) -> Result<Vec<u8>> {
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
    let p = brand::PREFIX;
    add(format!("{p}/"), &[], true)?;
    add(format!("{p}/pages/"), &[], true)?;
    add(format!("{p}/Caddyfile"), caddyfile(pages).as_bytes(), false)?;
    for page in PAGES {
        add(
            format!("{p}/pages/{}.html", page.file),
            render(page).as_bytes(),
            false,
        )?;
    }
    if !pages.custom.is_empty() {
        add(format!("{p}/pages/custom/"), &[], true)?;
    }
    let mut owners = Vec::new();
    for c in &pages.custom {
        let owner = c.service_id.as_deref().unwrap_or("instance");
        if !owners.contains(&owner) {
            owners.push(owner);
            add(format!("{p}/pages/custom/{owner}/"), &[], true)?;
        }
        add(format!("{p}/pages/{}", c.file()), c.html.as_bytes(), false)?;
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

    fn custom(service: Option<&str>, hosts: &[&str], kind: ErrorPageKind) -> CustomPage {
        CustomPage {
            service_id: service.map(Into::into),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            kind,
            html: format!("<p>{kind}</p>"),
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
            assert_eq!(common::check_error_page(&html), Ok(Vec::new()));
        }
        assert!(render(&STARTING).contains("http-equiv=\"refresh\""));
        assert!(!render(&NO_SERVER).contains("http-equiv"));
        assert!(!render(&NOTHING_HERE).contains("reload"));
    }

    #[test]
    fn the_caddyfile_holds_domains_without_a_container() {
        let file = caddyfile(&ProxyPages {
            held: vec![
                held("new.example.com", true, Hold::Starting),
                held("off.localhost", false, Hold::Down),
            ],
            ..Default::default()
        });
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
        let empty = caddyfile(&ProxyPages::default());
        assert!(!empty.contains("@starting") && !empty.contains("@raw"));
        assert_eq!(empty.matches("\t\t\ttemplates\n").count(), 4);
    }

    #[test]
    fn unbranded_hosts_get_the_raw_response_first() {
        let file = caddyfile(&ProxyPages {
            held: vec![held("new.example.com", true, Hold::Starting)],
            raw: vec!["raw.example.com".into(), "raw2.example.com".into()],
            ..Default::default()
        });
        let raw = file
            .find("\t\t@raw host raw.example.com raw2.example.com\n\t\thandle @raw {\n\t\t\trespond \"\" {err.status_code}\n\t\t}")
            .expect("raw branch");
        assert!(raw < file.find("@starting").unwrap());
    }

    #[test]
    fn custom_pages_win_over_the_instance_default_and_the_built_in_page() {
        let file = caddyfile(&ProxyPages {
            held: vec![
                held("app.example.com", true, Hold::Starting),
                held("other.example.com", true, Hold::Starting),
            ],
            custom: vec![
                custom(
                    Some("svc1"),
                    &["app.example.com", "www.example.com"],
                    ErrorPageKind::Starting,
                ),
                custom(Some("svc1"), &["app.example.com"], ErrorPageKind::Error),
                custom(Some("svc2"), &["api.example.com"], ErrorPageKind::NoServer),
                custom(None, &[], ErrorPageKind::NotFound),
            ],
            ..Default::default()
        });
        let at = |needle: &str| file.find(needle).unwrap_or_else(|| panic!("{needle}"));
        let svc_starting = at("@custom_0 host app.example.com\n");
        assert!(svc_starting < at("@starting host app.example.com other.example.com"));
        assert!(file.contains("rewrite * /custom/svc1/starting.html"));
        assert!(file.contains(
            "@custom_2 {\n\t\t\thost api.example.com\n\t\t\texpression `{err.status_code} in [502, 503]`"
        ));
        assert!(file.contains("expression `!({err.status_code} in [404, 502, 503, 504])`"));
        assert!(at("rewrite * /custom/svc2/no-server.html") < at("@missing"));
        let missing = &file[at("handle @missing {")..at("@down")];
        assert!(missing.contains("rewrite * /custom/instance/nothing-here.html"));
        assert!(!missing.contains("templates"));
        assert!(file[at("handle @down {")..].contains("templates"));
    }

    #[test]
    fn the_archive_unpacks_under_the_prefix() {
        let pages = ProxyPages {
            custom: vec![
                custom(Some("svc1"), &["a.example.com"], ErrorPageKind::NoServer),
                custom(Some("svc1"), &["a.example.com"], ErrorPageKind::Timeout),
                custom(None, &[], ErrorPageKind::NotFound),
            ],
            ..Default::default()
        };
        let bytes = archive(&pages).unwrap();
        let mut names: Vec<String> = tar::Archive::new(bytes.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        names.sort();
        let p = brand::PREFIX;
        assert!(names.contains(&format!("{p}/Caddyfile")));
        assert!(names.contains(&format!("{p}/pages/no-server.html")));
        assert!(names.contains(&format!("{p}/pages/custom/svc1/timed-out.html")));
        assert!(names.contains(&format!("{p}/pages/custom/instance/nothing-here.html")));
        assert_eq!(names.iter().filter(|n| n.ends_with(".html")).count(), 8);
        assert_eq!(
            names
                .iter()
                .filter(|n| n.trim_end_matches('/').ends_with("custom/svc1"))
                .count(),
            1
        );
    }
}
