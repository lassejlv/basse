//! The instance domain: the hostname the proxy serves the daemon's own API on with automatic
//! HTTPS, and the checks that guide an operator through pointing DNS at the server.
//!
//! The checks prove the path end to end instead of trusting IP addresses alone: the daemon
//! hands out a one-time token, fetches it back through the domain over http and https, and
//! only a request that really reached this server's proxy can answer with it. That works the
//! same behind Cloudflare's proxy (orange cloud), whose edge addresses the domain resolves to.

use crate::cloudflare::Ranges;
use anyhow::{Context as _, Result};
use common::{
    CheckStatus, CloudflareProxy, DnsRecord, DomainCheck, DomainCheckKind, InstanceDomain, brand,
};
use futures_util::future::BoxFuture;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Services that echo the caller's public address, tried in order.
const IPV4_ECHO: &str = "https://api.ipify.org";
const IPV6_ECHO: &str = "https://api6.ipify.org";

/// Header the proxy sets on what it forwards to the daemon: the scheme it received it on.
pub fn scheme_header() -> String {
    format!("x-{}-origin-scheme", brand::PREFIX)
}

/// Path prefix of the verification token. It sits under the ACME HTTP-01 path, so the check
/// takes exactly the route a Let's Encrypt challenge takes through Cloudflare or any other
/// proxy; the proxy answers real challenges itself and forwards everything else here.
pub fn verify_prefix() -> String {
    format!("/.well-known/acme-challenge/{}-verify-", brand::PREFIX)
}

pub fn verify_path(nonce: &str) -> String {
    format!("{}{nonce}", verify_prefix())
}

/// A fresh one-time token.
pub fn new_nonce() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// The verification route's answer to the last path segment `token`: the nonce of a check
/// running now and the scheme the proxy says it received the request on.
pub fn verify_answer(token: &str, active: &[String], scheme: Option<&str>) -> Option<String> {
    let nonce = token.strip_prefix(&format!("{}-verify-", brand::PREFIX))?;
    active
        .iter()
        .any(|a| a == nonce)
        .then(|| format!("{nonce} {}", scheme.unwrap_or("unknown")))
}

/// Lowercases and trims a domain the operator typed, and rejects anything that is not a
/// plain hostname: schemes, paths, ports, IP addresses and single labels.
pub fn normalize(input: &str) -> Result<String, String> {
    let d = input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let labels: Vec<&str> = d.split('.').collect();
    if d.parse::<IpAddr>().is_ok()
        || d.len() > 253
        || labels.len() < 2
        || !labels.iter().all(|l| label_ok(l))
        || labels
            .last()
            .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(format!(
            "'{}' is not a domain: use a hostname such as deploy.example.com",
            input.trim()
        ));
    }
    Ok(d)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// One GET request a check makes.
#[derive(Debug, Clone, Copy)]
pub struct Get<'a> {
    pub scheme: Scheme,
    pub domain: &'a str,
    pub path: &'a str,
    /// Connect to this address instead of what DNS answers, still sending the domain as host
    /// and TLS server name.
    pub via: Option<IpAddr>,
}

/// Where the checks reach out to the world, so tests can stand in for DNS and the network.
pub trait Probe: Send + Sync {
    /// The addresses `host` resolves to.
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>>>;
    /// Opens and closes a TCP connection.
    fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, Result<()>>;
    /// The request, with certificate verification and without following redirects.
    fn get<'a>(&'a self, req: Get<'a>) -> BoxFuture<'a, Result<Reply, FetchError>>;
    /// This server's public IPv4 and IPv6 address as the internet sees it.
    fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The TLS handshake failed: no certificate yet, or one that does not match.
    Tls(String),
    /// This server has no route to the address family, typically IPv6.
    NoRoute(String),
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tls(e) | Self::NoRoute(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

/// The real network.
pub struct NetProbe {
    http: reqwest::Client,
    /// Extra trusted roots, for tests against the proxy's local CA.
    roots: Vec<reqwest::Certificate>,
}

impl NetProbe {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Self::builder(&[]).build()?,
            roots: Vec::new(),
        })
    }

    /// Also trusts the CA certificate in `pem`.
    #[cfg(test)]
    pub fn with_root(pem: &[u8]) -> Result<Self> {
        let root = reqwest::Certificate::from_pem(pem)?;
        Ok(Self {
            http: Self::builder(std::slice::from_ref(&root)).build()?,
            roots: vec![root],
        })
    }

    fn builder(roots: &[reqwest::Certificate]) -> reqwest::ClientBuilder {
        roots.iter().fold(
            reqwest::Client::builder()
                .timeout(PROBE_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none()),
            |b, root| b.add_root_certificate(root.clone()),
        )
    }

    async fn echo(&self, url: &str) -> Option<IpAddr> {
        let text = self.http.get(url).send().await.ok()?.text().await.ok()?;
        text.trim().parse().ok()
    }
}

impl Probe for NetProbe {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let addrs = tokio::time::timeout(PROBE_TIMEOUT, tokio::net::lookup_host((host, 0)))
                .await
                .context("DNS lookup timed out")??;
            let mut ips: Vec<IpAddr> = Vec::new();
            for ip in addrs.map(|a| a.ip()) {
                if !ips.contains(&ip) {
                    ips.push(ip);
                }
            }
            Ok(ips)
        })
    }

    fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr))
                .await
                .context("timed out")??;
            Ok(())
        })
    }

    fn get<'a>(&'a self, req: Get<'a>) -> BoxFuture<'a, Result<Reply, FetchError>> {
        Box::pin(async move {
            let mut builder = Self::builder(&self.roots);
            if let Some(ip) = req.via {
                builder = builder.resolve(req.domain, SocketAddr::new(ip, 0));
            }
            let client = builder
                .build()
                .map_err(|e| FetchError::Other(error_chain(&e)))?;
            let url = format!("{}://{}{}", req.scheme.as_str(), req.domain, req.path);
            let res = client.get(url).send().await.map_err(|e| {
                let chain = error_chain(&e);
                let lower = chain.to_ascii_lowercase();
                if no_route(&e) {
                    FetchError::NoRoute(chain)
                } else if ["certificate", "tls", "handshake", "alert"]
                    .iter()
                    .any(|w| lower.contains(w))
                {
                    FetchError::Tls(chain)
                } else {
                    FetchError::Other(chain)
                }
            })?;
            let status = res.status().as_u16();
            let location = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = res.text().await.unwrap_or_default();
            Ok(Reply {
                status,
                body,
                location,
            })
        })
    }

    fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)> {
        Box::pin(async move {
            let (v4, v6) = tokio::join!(self.echo(IPV4_ECHO), self.echo(IPV6_ECHO));
            (v4.filter(IpAddr::is_ipv4), v6.filter(IpAddr::is_ipv6))
        })
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = s.source();
    }
    parts.join(": ")
}

/// Whether the error says this machine cannot route to the address at all.
fn no_route(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(e);
    while let Some(s) = source {
        if let Some(io) = s.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::NetworkUnreachable | std::io::ErrorKind::AddrNotAvailable
            )
        {
            return true;
        }
        source = s.source();
    }
    false
}

/// The records to create: an A record for the IPv4 address and an AAAA one for IPv6.
pub fn records(domain: &str, v4: Option<IpAddr>, v6: Option<IpAddr>) -> Vec<DnsRecord> {
    [("A", v4), ("AAAA", v6)]
        .into_iter()
        .filter_map(|(kind, ip)| {
            ip.map(|ip| DnsRecord {
                kind: kind.into(),
                name: domain.to_string(),
                value: ip.to_string(),
            })
        })
        .collect()
}

fn check(
    kind: DomainCheckKind,
    status: CheckStatus,
    detail: String,
    hint: Option<String>,
) -> DomainCheck {
    DomainCheck {
        kind,
        status,
        detail,
        hint,
    }
}

fn skipped(kind: DomainCheckKind) -> DomainCheck {
    check(
        kind,
        CheckStatus::Skipped,
        "waits for the steps above".into(),
        None,
    )
}

fn passed(checks: &[DomainCheck], kind: DomainCheckKind) -> bool {
    checks
        .iter()
        .any(|c| c.kind == kind && c.status == CheckStatus::Pass)
}

/// Where one address the domain resolves to leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// This server's own public address (or any address, while that is unknown).
    Direct,
    /// Cloudflare's edge, which forwards to the record's origin.
    Cloudflare,
    /// Neither: a stale record, NAT, or another proxy. The token decides.
    Unknown,
}

fn record_type(ip: IpAddr) -> &'static str {
    if ip.is_ipv4() { "A" } else { "AAAA" }
}

fn family(ip: IpAddr) -> &'static str {
    if ip.is_ipv4() { "IPv4" } else { "IPv6" }
}

/// What fetching the token back produced.
#[derive(Debug, Clone)]
enum Outcome {
    /// The token came back; the proxy received the request over this scheme.
    Reached(String),
    /// Something answered, but not with the token.
    Answered(Reply),
    Failed(FetchError),
}

async fn verify(
    probe: &dyn Probe,
    scheme: Scheme,
    domain: &str,
    nonce: &str,
    via: IpAddr,
) -> Outcome {
    let path = verify_path(nonce);
    let req = Get {
        scheme,
        domain,
        path: &path,
        via: Some(via),
    };
    match probe.get(req).await {
        Ok(reply) if reply.status == 200 => match reply.body.trim().split_once(' ') {
            Some((token, received)) if token == nonce => Outcome::Reached(received.to_string()),
            _ => Outcome::Answered(reply),
        },
        Ok(reply) => Outcome::Answered(reply),
        Err(e) => Outcome::Failed(e),
    }
}

/// A Cloudflare error page status, as the reason it gives.
fn cloudflare_error(status: u16) -> Option<&'static str> {
    Some(match status {
        520 => "this server answered with something unexpected",
        521 => "this server refused the connection",
        522 => "the connection to this server timed out",
        523 => "this server's address is unreachable",
        524 => "this server took too long to answer",
        525 => "the TLS handshake with this server failed",
        526 => "this server's certificate is not valid",
        _ => return None,
    })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

const FULL_STRICT: &str = "set Cloudflare's SSL/TLS encryption mode to Full (strict); Flexible sends requests here over http, which redirects to https in a loop";

/// The result of the last check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<DomainCheck>,
    pub cloudflare: Option<CloudflareProxy>,
}

/// Runs the five checks in order. A check whose prerequisites failed is skipped. `nonce` must
/// be answered by this daemon's verification route while the checks run.
pub async fn run_checks(
    probe: &dyn Probe,
    domain: &str,
    public: &[IpAddr],
    cloudflare_ranges: &Ranges,
    nonce: &str,
) -> Report {
    use DomainCheckKind::*;
    let mut checks = Vec::new();
    let public_list = public
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");

    let resolved = match probe.resolve(domain).await {
        Ok(ips) if !ips.is_empty() => ips,
        Ok(_) | Err(_) => {
            checks.push(check(
                Dns,
                CheckStatus::Fail,
                format!("{domain} does not resolve"),
                Some(format!(
                    "create the records above at your DNS provider; new records can take a few minutes to appear{}",
                    if public.is_empty() { String::new() } else { format!(" (value {public_list})") }
                )),
            ));
            checks.extend([Http, Https, Certificate, Api].map(skipped));
            return Report {
                checks,
                cloudflare: None,
            };
        }
    };
    let route = |ip: IpAddr| {
        if cloudflare_ranges.contains(ip) {
            Route::Cloudflare
        } else if public.is_empty() || public.contains(&ip) {
            Route::Direct
        } else {
            Route::Unknown
        }
    };
    // One address per family and route: each is a separate way in, tested on its own.
    let mut paths: Vec<(IpAddr, Route)> = Vec::new();
    for &ip in &resolved {
        let r = route(ip);
        if !paths
            .iter()
            .any(|&(p, pr)| p.is_ipv4() == ip.is_ipv4() && pr == r)
        {
            paths.push((ip, r));
        }
    }
    let types = |proxied: bool| -> Vec<String> {
        ["A", "AAAA"]
            .into_iter()
            .filter(|t| {
                paths
                    .iter()
                    .any(|&(ip, r)| (r == Route::Cloudflare) == proxied && record_type(ip) == *t)
            })
            .map(str::to_string)
            .collect()
    };
    let mut cloudflare =
        paths
            .iter()
            .any(|&(_, r)| r == Route::Cloudflare)
            .then(|| CloudflareProxy {
                records: types(true),
                direct: types(false),
                flexible: false,
            });

    let mut http = Vec::new();
    for &(ip, r) in &paths {
        http.push((ip, r, verify(probe, Scheme::Http, domain, nonce, ip).await));
    }

    // DNS: every record must lead here, directly or through Cloudflare.
    let summary = dns_summary(&resolved, &route, &http, !public.is_empty());
    let stray: Vec<IpAddr> = http
        .iter()
        .filter(|(_, r, o)| {
            *r == Route::Unknown
                && !matches!(
                    o,
                    Outcome::Reached(_) | Outcome::Failed(FetchError::NoRoute(_))
                )
        })
        .map(|(ip, _, _)| *ip)
        .collect();
    if !stray.is_empty() {
        let types: Vec<&str> = ["A", "AAAA"]
            .into_iter()
            .filter(|t| stray.iter().any(|ip| record_type(*ip) == *t))
            .collect();
        checks.push(check(
            Dns,
            CheckStatus::Fail,
            format!("{summary}, not this server ({public_list})"),
            Some(format!(
                "point the {} record at {public_list}; Cloudflare's proxy (orange cloud) can stay on",
                types.join(" and ")
            )),
        ));
        checks.extend([Http, Https, Certificate, Api].map(skipped));
        return Report { checks, cloudflare };
    }
    checks.push(check(
        Dns,
        CheckStatus::Pass,
        if public.is_empty() {
            format!("{summary}; this server's public address is unknown, so it was not compared")
        } else {
            summary
        },
        None,
    ));

    // Port 80: the token must come back over plain http on every way in, because that is the
    // way Let's Encrypt's HTTP-01 challenge takes.
    let mut http_ok = Vec::new();
    let mut untested = Vec::new();
    let mut http_fail = None;
    for (ip, r, outcome) in &http {
        match outcome {
            Outcome::Reached(_) => http_ok.push((*ip, *r)),
            Outcome::Failed(FetchError::NoRoute(_)) => untested.push(*ip),
            other => {
                http_fail = Some(http_failure(domain, *ip, *r, other, &public_list));
                break;
            }
        }
    }
    checks.push(match http_fail {
        Some((detail, hint)) => check(Http, CheckStatus::Fail, detail, Some(hint)),
        None if http_ok.is_empty() => check(
            Http,
            CheckStatus::Fail,
            format!("this server cannot reach {domain} at all: no route to its addresses"),
            Some("check this server's own network connection".into()),
        ),
        None => check(
            Http,
            CheckStatus::Pass,
            format!(
                "http://{domain} reaches this server{}",
                ways(&http_ok, &untested)
            ),
            None,
        ),
    });

    // Port 443: directly, a TCP connection (the certificate may not exist yet); through
    // Cloudflare, the token over https, which also shows how Cloudflare talks to this server.
    let mut https_ok = Vec::new();
    let mut https_fail = None;
    for &(ip, r) in paths.iter().filter(|(ip, _)| !untested.contains(ip)) {
        let result = match r {
            Route::Cloudflare => {
                let outcome = verify(probe, Scheme::Https, domain, nonce, ip).await;
                cloudflare_https(domain, &outcome)
            }
            Route::Direct | Route::Unknown => match probe.connect(SocketAddr::new(ip, 443)).await {
                Ok(()) => Ok(()),
                Err(e) => Err(HttpsFail::Port(format!(
                    "cannot connect to {ip}:443: {e:#}"
                ))),
            },
        };
        match result {
            Ok(()) => https_ok.push((ip, r)),
            Err(e) => {
                https_fail = Some(e);
                break;
            }
        }
    }
    checks.push(match https_fail {
        None => check(
            Https,
            CheckStatus::Pass,
            format!("https://{domain} reaches this server{}", ways(&https_ok, &untested)),
            None,
        ),
        Some(HttpsFail::Flexible(detail)) => {
            if let Some(cf) = cloudflare.as_mut() {
                cf.flexible = true;
            }
            check(Https, CheckStatus::Fail, detail, Some(FULL_STRICT.into()))
        }
        Some(HttpsFail::Port(detail)) => check(
            Https,
            CheckStatus::Fail,
            detail,
            Some(
                "open port 443/tcp in the server's and the provider's firewall (ufw allow 443/tcp) and make sure the proxy runs"
                    .into(),
            ),
        ),
        Some(HttpsFail::Edge(detail, hint)) => check(Https, CheckStatus::Fail, detail, Some(hint)),
    });
    if !(passed(&checks, Http) && passed(&checks, Https)) {
        checks.extend([Certificate, Api].map(skipped));
        return Report { checks, cloudflare };
    }

    // The certificate this server serves: through a direct address when there is one, else on
    // this machine, since visitors behind Cloudflare only ever see Cloudflare's certificate.
    let direct = https_ok
        .iter()
        .find(|(_, r)| *r != Route::Cloudflare)
        .map(|(ip, _)| *ip);
    let origin = direct.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let reply = probe
        .get(Get {
            scheme: Scheme::Https,
            domain,
            path: "/healthz",
            via: Some(origin),
        })
        .await;
    let reply = match reply {
        Err(FetchError::Tls(e)) => {
            checks.push(check(
                Certificate,
                CheckStatus::Fail,
                format!("no valid certificate yet: {e}"),
                Some(format!(
                    "the proxy requests one from Let's Encrypt as soon as DNS and port 80 work; check again in a minute, and see docker logs {} if it keeps failing",
                    brand::PROXY_CONTAINER
                )),
            ));
            checks.push(skipped(Api));
            return Report { checks, cloudflare };
        }
        Err(e) => {
            checks.push(check(
                Certificate,
                CheckStatus::Fail,
                format!("https://{domain} did not answer on {origin}: {e}"),
                Some("check again in a minute; the proxy may still be starting".into()),
            ));
            checks.push(skipped(Api));
            return Report { checks, cloudflare };
        }
        Ok(reply) => reply,
    };
    checks.push(check(
        Certificate,
        CheckStatus::Pass,
        if cloudflare.is_some() {
            "valid on this server; visitors through Cloudflare see Cloudflare's".into()
        } else {
            format!("https://{domain} serves a valid certificate")
        },
        None,
    ));

    // The API on every way in.
    let mut api_fail = None;
    if direct.is_some() {
        api_fail = api_failure(domain, &reply);
    }
    if api_fail.is_none()
        && let Some(&(ip, _)) = https_ok.iter().find(|(_, r)| *r == Route::Cloudflare)
    {
        let through = probe
            .get(Get {
                scheme: Scheme::Https,
                domain,
                path: "/healthz",
                via: Some(ip),
            })
            .await;
        api_fail = match through {
            Ok(reply) => api_failure(domain, &reply),
            Err(e) => Some(ApiFail {
                detail: format!("https://{domain}/healthz through Cloudflare failed: {e}"),
                hint: "check again in a minute".into(),
                looping: false,
            }),
        };
    }
    checks.push(match api_fail {
        None => check(
            Api,
            CheckStatus::Pass,
            format!("https://{domain} reaches the API"),
            None,
        ),
        Some(ApiFail {
            detail,
            hint,
            looping,
        }) => {
            if looping && let Some(cf) = cloudflare.as_mut() {
                cf.flexible = true;
            }
            check(Api, CheckStatus::Fail, detail, Some(hint))
        }
    });
    Report { checks, cloudflare }
}

/// `A → this server · AAAA → Cloudflare proxy`, or `proxied by Cloudflare` when every record is.
fn dns_summary(
    resolved: &[IpAddr],
    route: &dyn Fn(IpAddr) -> Route,
    http: &[(IpAddr, Route, Outcome)],
    public_known: bool,
) -> String {
    if resolved.iter().all(|ip| route(*ip) == Route::Cloudflare) {
        return "proxied by Cloudflare".into();
    }
    let reached = |ip: IpAddr| {
        http.iter()
            .any(|(p, _, o)| *p == ip && matches!(o, Outcome::Reached(_)))
    };
    ["A", "AAAA"]
        .into_iter()
        .filter_map(|t| {
            let mut targets: Vec<String> = Vec::new();
            for &ip in resolved.iter().filter(|ip| record_type(**ip) == t) {
                let target = match route(ip) {
                    Route::Cloudflare => "Cloudflare proxy".to_string(),
                    Route::Direct if public_known => "this server".to_string(),
                    Route::Direct => ip.to_string(),
                    Route::Unknown if reached(ip) => format!("{ip}, which reaches this server"),
                    Route::Unknown => ip.to_string(),
                };
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            (!targets.is_empty()).then(|| format!("{t} → {}", targets.join(", ")))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `: IPv4 directly · IPv6 through Cloudflare`, naming families that could not be tested.
fn ways(ok: &[(IpAddr, Route)], untested: &[IpAddr]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for &(ip, r) in ok {
        let how = if r == Route::Cloudflare {
            "through Cloudflare"
        } else {
            "directly"
        };
        let part = format!("{} {how}", family(ip));
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    let single = parts.len() == 1 && untested.is_empty();
    let mut out = if single {
        if ok.iter().any(|(_, r)| *r == Route::Cloudflare) {
            " through Cloudflare".to_string()
        } else {
            String::new()
        }
    } else {
        format!(": {}", parts.join(" · "))
    };
    for ip in untested {
        out.push_str(&format!(
            " · {} not tested, this server has no {} route",
            family(*ip),
            family(*ip)
        ));
    }
    out
}

fn http_failure(
    domain: &str,
    ip: IpAddr,
    route: Route,
    outcome: &Outcome,
    public_list: &str,
) -> (String, String) {
    let firewall = format!(
        "open port 80/tcp in the server's and the provider's firewall (ufw allow 80/tcp) and make sure the proxy runs; some providers block a server from reaching its own public address, so also try curl -I http://{domain} from another machine"
    );
    let reinstall = format!(
        "reinstall the proxy so it routes the domain here: {} instance domain set {domain}",
        brand::CLI_BIN
    );
    match (route, outcome) {
        (Route::Cloudflare, Outcome::Answered(r))
            if is_redirect(r.status)
                && r.location.as_deref().is_some_and(|l| l.starts_with("https://")) =>
        {
            (
                format!("Cloudflare redirects http://{domain} to https before it reaches this server"),
                "turn off Always Use HTTPS (SSL/TLS → Edge Certificates) or exempt /.well-known/acme-challenge/* with a Configuration Rule: Let's Encrypt validates the certificate over plain http".into(),
            )
        }
        (Route::Cloudflare, Outcome::Answered(r)) if cloudflare_error(r.status).is_some() => (
            format!(
                "Cloudflare cannot reach this server on port 80 ({}: {})",
                r.status,
                cloudflare_error(r.status).unwrap_or_default()
            ),
            if public_list.is_empty() {
                firewall
            } else {
                format!("the proxied record must hold {public_list}; {firewall}")
            },
        ),
        (Route::Cloudflare, Outcome::Answered(r)) => (
            format!(
                "http://{domain} answered {} through Cloudflare without the verification token",
                r.status
            ),
            format!("check that the proxied record holds this server's address; {reinstall}"),
        ),
        (_, Outcome::Answered(r)) => (
            format!(
                "http://{domain} on {ip} answered {} without the verification token",
                r.status
            ),
            reinstall,
        ),
        (Route::Cloudflare, Outcome::Failed(e)) => (
            format!("http://{domain} through Cloudflare failed: {e}"),
            "check again in a minute".into(),
        ),
        (_, Outcome::Failed(e)) => (format!("cannot reach {ip}:80: {e}"), firewall),
        (_, Outcome::Reached(_)) => unreachable!("a reached path is not a failure"),
    }
}

enum HttpsFail {
    Port(String),
    Flexible(String),
    /// Detail and hint.
    Edge(String, String),
}

fn cloudflare_https(domain: &str, outcome: &Outcome) -> Result<(), HttpsFail> {
    match outcome {
        Outcome::Reached(received) if received == "https" => Ok(()),
        Outcome::Reached(_) => Err(HttpsFail::Flexible(
            "Cloudflare forwards https requests to this server over plain http (SSL/TLS mode Flexible)".into(),
        )),
        // Cloudflare got through to port 443; the certificate check below says what is missing.
        Outcome::Answered(r) if matches!(r.status, 525 | 526) => Ok(()),
        Outcome::Answered(r) if cloudflare_error(r.status).is_some() => Err(HttpsFail::Port(format!(
            "Cloudflare cannot reach this server on port 443 ({}: {})",
            r.status,
            cloudflare_error(r.status).unwrap_or_default()
        ))),
        Outcome::Answered(r)
            if is_redirect(r.status)
                && r.location.as_deref().is_some_and(|l| l.starts_with(&format!("https://{domain}/"))) =>
        {
            Err(HttpsFail::Flexible(format!(
                "https://{domain} redirects to itself: a redirect loop between Cloudflare and this server"
            )))
        }
        Outcome::Answered(r) => Err(HttpsFail::Edge(
            format!("https://{domain} answered {} through Cloudflare without the verification token", r.status),
            "check that the proxied record holds this server's address".into(),
        )),
        Outcome::Failed(FetchError::Tls(e)) => Err(HttpsFail::Edge(
            format!("Cloudflare's own certificate for {domain} is not valid yet: {e}"),
            "Cloudflare issues it within minutes of the record being proxied; see SSL/TLS → Edge Certificates".into(),
        )),
        Outcome::Failed(e) => Err(HttpsFail::Edge(
            format!("https://{domain} through Cloudflare failed: {e}"),
            "check again in a minute".into(),
        )),
    }
}

struct ApiFail {
    detail: String,
    hint: String,
    /// The API redirected to itself, which behind Cloudflare means the Flexible SSL/TLS mode.
    looping: bool,
}

fn api_failure(domain: &str, reply: &Reply) -> Option<ApiFail> {
    if reply.status == 200 && reply.body.trim() == "ok" {
        return None;
    }
    let looping = is_redirect(reply.status)
        && reply.location.as_deref() == Some(format!("https://{domain}/healthz").as_str());
    Some(if looping {
        ApiFail {
            detail: format!("https://{domain}/healthz redirects to itself: a redirect loop"),
            hint: FULL_STRICT.into(),
            looping,
        }
    } else {
        ApiFail {
            detail: format!("https://{domain}/healthz answered {}", reply.status),
            hint: format!(
                "reinstall the proxy so it routes the domain to the API: {} instance domain set {domain}",
                brand::CLI_BIN
            ),
            looping,
        }
    })
}

/// Whether every check passed.
pub fn all_passed(checks: &[DomainCheck]) -> bool {
    checks.len() == DomainCheckKind::ALL.len()
        && checks.iter().all(|c| c.status == CheckStatus::Pass)
}

/// The API response for the configured domain.
pub fn status(
    domain: Option<&str>,
    verified_at: Option<common::Timestamp>,
    v4: Option<IpAddr>,
    v6: Option<IpAddr>,
    last: Option<(Report, common::Timestamp)>,
) -> InstanceDomain {
    let Some(domain) = domain else {
        return InstanceDomain {
            public_ipv4: v4.map(|ip| ip.to_string()),
            public_ipv6: v6.map(|ip| ip.to_string()),
            ..Default::default()
        };
    };
    let (report, checked_at) = match last {
        Some((report, at)) => (report, Some(at)),
        None => (Report::default(), None),
    };
    InstanceDomain {
        domain: Some(domain.to_string()),
        url: verified_at.map(|_| format!("https://{domain}")),
        verified: verified_at.is_some(),
        verified_at,
        public_ipv4: v4.map(|ip| ip.to_string()),
        public_ipv6: v6.map(|ip| ip.to_string()),
        records: records(domain, v4, v6),
        checks: report.checks,
        checked_at,
        cloudflare: report.cloudflare,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    type Web = Box<dyn Fn(&Get) -> Result<Reply, FetchError> + Send + Sync>;

    /// A network where DNS answers, open ports and web replies are set by the test.
    #[derive(Default)]
    pub struct FakeProbe {
        pub dns: HashMap<String, Vec<IpAddr>>,
        pub open: Vec<u16>,
        /// Answers GET requests; without it nothing answers.
        pub web: Option<Web>,
        pub public: (Option<IpAddr>, Option<IpAddr>),
        /// Every GET made, as `scheme path via`.
        pub seen: Mutex<Vec<String>>,
    }

    impl Probe for FakeProbe {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>>> {
            Box::pin(async move { self.dns.get(host).cloned().context("NXDOMAIN") })
        }

        fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, Result<()>> {
            Box::pin(async move {
                if self.open.contains(&addr.port()) {
                    Ok(())
                } else {
                    anyhow::bail!("connection refused")
                }
            })
        }

        fn get<'a>(&'a self, req: Get<'a>) -> BoxFuture<'a, Result<Reply, FetchError>> {
            Box::pin(async move {
                self.seen.lock().unwrap().push(format!(
                    "{} {} {}",
                    req.scheme.as_str(),
                    req.path,
                    req.via.map(|ip| ip.to_string()).unwrap_or_default()
                ));
                match &self.web {
                    Some(web) => web(&req),
                    None => Err(FetchError::Other("connection refused".into())),
                }
            })
        }

        fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)> {
            Box::pin(async move { self.public })
        }
    }

    const D: &str = "deploy.example.com";
    const NONCE: &str = "0123abcd";

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn statuses(checks: &[DomainCheck]) -> Vec<CheckStatus> {
        checks.iter().map(|c| c.status).collect()
    }

    fn ok(status: u16, body: &str) -> Result<Reply, FetchError> {
        Ok(Reply {
            status,
            body: body.into(),
            location: None,
        })
    }

    fn redirect(to: String) -> Result<Reply, FetchError> {
        Ok(Reply {
            status: 301,
            body: String::new(),
            location: Some(to),
        })
    }

    /// This server's proxy: echoes the token with the scheme it arrived on, and serves the API
    /// over https once it has a certificate.
    fn origin(req: &Get, scheme: Scheme, cert: bool) -> Result<Reply, FetchError> {
        if let Some(token) = req.path.strip_prefix(&verify_prefix()) {
            return ok(200, &format!("{token} {}", scheme.as_str()));
        }
        match (scheme, cert) {
            (Scheme::Https, false) => Err(FetchError::Tls("tlsv1 alert internal error".into())),
            (Scheme::Https, true) => ok(200, "ok"),
            (Scheme::Http, _) => redirect(format!("https://{}{}", req.domain, req.path)),
        }
    }

    /// How the zone's SSL/TLS mode makes Cloudflare talk to the origin.
    #[derive(Clone, Copy)]
    enum Mode {
        FullStrict,
        Flexible,
    }

    /// Cloudflare's edge in front of `origin`, for requests to its addresses.
    fn edge(req: &Get, mode: Mode, cert: bool) -> Result<Reply, FetchError> {
        match (req.scheme, mode) {
            (Scheme::Http, _) => origin(req, Scheme::Http, cert),
            (Scheme::Https, Mode::Flexible) => match origin(req, Scheme::Http, cert) {
                Ok(r) if r.location.is_some() => redirect(r.location.unwrap()),
                other => other,
            },
            (Scheme::Https, Mode::FullStrict) if !cert => {
                ok(525, "<html>SSL handshake failed</html>")
            }
            (Scheme::Https, Mode::FullStrict) => origin(req, Scheme::Https, cert),
        }
    }

    fn cloudflare(ip: Option<IpAddr>) -> bool {
        ip.is_some_and(|ip| Ranges::bundled().contains(ip))
    }

    /// Direct addresses reach the origin, Cloudflare's reach the edge.
    fn network(mode: Mode, cert: bool) -> Web {
        Box::new(move |req| {
            if cloudflare(req.via) {
                edge(req, mode, cert)
            } else {
                origin(req, req.scheme, cert)
            }
        })
    }

    async fn run(probe: &FakeProbe, public: &[&str]) -> Report {
        let public: Vec<IpAddr> = public.iter().map(|s| ip(s)).collect();
        run_checks(probe, D, &public, &Ranges::bundled(), NONCE).await
    }

    fn resolving(ips: &[&str]) -> HashMap<String, Vec<IpAddr>> {
        HashMap::from([(D.to_string(), ips.iter().map(|s| ip(s)).collect())])
    }

    #[test]
    fn domains_are_normalized_and_validated() {
        assert_eq!(normalize(" https://Deploy.Example.com/ ").unwrap(), D);
        assert_eq!(normalize("deploy.example.com.").unwrap(), D);
        for bad in [
            "localhost",
            "203.0.113.4",
            "-a.com",
            "a..com",
            "a.com:8080",
            "a.com/x",
            "a.123",
            "",
        ] {
            assert!(normalize(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn the_verification_route_answers_only_running_checks() {
        let active = vec![NONCE.to_string()];
        let token = format!("{}-verify-{NONCE}", brand::PREFIX);
        assert_eq!(
            verify_answer(&token, &active, Some("http")).as_deref(),
            Some("0123abcd http")
        );
        assert_eq!(
            verify_answer(&token, &active, None).as_deref(),
            Some("0123abcd unknown")
        );
        assert_eq!(verify_answer(&token, &[], Some("https")), None);
        assert_eq!(verify_answer(NONCE, &active, Some("https")), None);
        assert_eq!(verify_path(NONCE), format!("{}{NONCE}", verify_prefix()));
    }

    #[test]
    fn records_cover_both_address_families() {
        let r = records(D, Some(ip("203.0.113.4")), Some(ip("2001:db8::4")));
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].kind.as_str(), r[0].value.as_str()),
            ("A", "203.0.113.4")
        );
        assert_eq!(
            (r[1].kind.as_str(), r[1].value.as_str()),
            ("AAAA", "2001:db8::4")
        );
        assert!(records(D, None, None).is_empty());
    }

    #[tokio::test]
    async fn a_missing_record_fails_dns_and_skips_the_rest() {
        let probe = FakeProbe::default();
        let report = run(&probe, &["203.0.113.4"]).await;
        assert_eq!(
            statuses(&report.checks),
            [
                CheckStatus::Fail,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
                CheckStatus::Skipped
            ]
        );
        assert!(
            report.checks[0]
                .hint
                .as_deref()
                .unwrap()
                .contains("203.0.113.4")
        );
    }

    #[tokio::test]
    async fn a_record_pointing_elsewhere_names_both_addresses() {
        let probe = FakeProbe {
            dns: resolving(&["198.51.100.9"]),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        let dns = &report.checks[0];
        assert_eq!(dns.status, CheckStatus::Fail);
        assert!(dns.detail.contains("198.51.100.9"), "{dns:?}");
        assert!(dns.detail.contains("203.0.113.4"), "{dns:?}");
        assert!(dns.hint.as_deref().unwrap().contains("A record"));
        assert!(report.cloudflare.is_none());
    }

    #[tokio::test]
    async fn an_unknown_address_that_returns_the_token_reaches_this_server() {
        let probe = FakeProbe {
            dns: resolving(&["198.51.100.9"]),
            open: vec![443],
            web: Some(network(Mode::FullStrict, true)),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        assert!(all_passed(&report.checks), "{:?}", report.checks);
        assert_eq!(
            report.checks[0].detail,
            "A → 198.51.100.9, which reaches this server"
        );
    }

    #[tokio::test]
    async fn closed_ports_fail_individually() {
        let probe = FakeProbe {
            dns: resolving(&["203.0.113.4"]),
            open: vec![80],
            web: Some(network(Mode::FullStrict, false)),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        assert_eq!(
            statuses(&report.checks),
            [
                CheckStatus::Pass,
                CheckStatus::Pass,
                CheckStatus::Fail,
                CheckStatus::Skipped,
                CheckStatus::Skipped
            ]
        );
        assert!(report.checks[2].hint.as_deref().unwrap().contains("443"));
    }

    #[tokio::test]
    async fn port_80_fails_when_the_token_does_not_come_back() {
        let probe = FakeProbe {
            dns: resolving(&["203.0.113.4"]),
            open: vec![443],
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        assert_eq!(report.checks[1].status, CheckStatus::Fail);
        assert!(report.checks[1].detail.contains("203.0.113.4:80"));
        assert!(report.checks[1].hint.as_deref().unwrap().contains("80/tcp"));
    }

    #[tokio::test]
    async fn a_missing_certificate_is_reported_with_the_tls_error() {
        let probe = FakeProbe {
            dns: resolving(&["203.0.113.4"]),
            open: vec![80, 443],
            web: Some(network(Mode::FullStrict, false)),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        assert_eq!(report.checks[3].status, CheckStatus::Fail);
        assert!(report.checks[3].detail.contains("alert"));
        assert_eq!(report.checks[4].status, CheckStatus::Skipped);
        assert!(!all_passed(&report.checks));
    }

    #[tokio::test]
    async fn everything_passing_verifies_the_domain() {
        let probe = FakeProbe {
            dns: resolving(&["2001:db8::4", "203.0.113.4"]),
            open: vec![80, 443],
            web: Some(network(Mode::FullStrict, true)),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4", "2001:db8::4"]).await;
        assert!(all_passed(&report.checks), "{:?}", report.checks);
        assert_eq!(
            report.checks[0].detail,
            "A → this server · AAAA → this server"
        );
        assert!(report.cloudflare.is_none());
        let s = status(
            Some(D),
            Some(chrono::Utc::now()),
            Some(ip("203.0.113.4")),
            None,
            Some((report, chrono::Utc::now())),
        );
        assert_eq!(s.url.as_deref(), Some("https://deploy.example.com"));
    }

    #[tokio::test]
    async fn a_proxy_that_does_not_route_to_the_api_fails_the_last_check() {
        let probe = FakeProbe {
            dns: resolving(&["203.0.113.4"]),
            open: vec![80, 443],
            web: Some(Box::new(|req| {
                if req.path == "/healthz" {
                    ok(502, "")
                } else {
                    origin(req, req.scheme, true)
                }
            })),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4"]).await;
        assert_eq!(report.checks[3].status, CheckStatus::Pass);
        assert_eq!(report.checks[4].status, CheckStatus::Fail);
        assert!(report.checks[4].detail.contains("502"));
    }

    #[tokio::test]
    async fn a_domain_proxied_by_cloudflare_passes_through_the_proxy() {
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1", "172.67.201.58", "2606:4700:3034::ac43:c93a"]),
            web: Some(network(Mode::FullStrict, true)),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        assert!(all_passed(&report.checks), "{:?}", report.checks);
        assert_eq!(report.checks[0].detail, "proxied by Cloudflare");
        assert_eq!(
            report.checks[1].detail,
            "http://deploy.example.com reaches this server: IPv4 through Cloudflare · IPv6 through Cloudflare"
        );
        assert_eq!(
            report.cloudflare,
            Some(CloudflareProxy {
                records: vec!["A".into(), "AAAA".into()],
                direct: Vec::new(),
                flexible: false
            })
        );
        let seen = probe.seen.lock().unwrap();
        assert!(
            seen.contains(&"https /healthz 127.0.0.1".to_string()),
            "the origin certificate is checked on this machine: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_mixed_domain_reports_each_record_type_and_tests_both_ways_in() {
        // Lasse's instance on v0.4.0: A straight at the server, AAAA through Cloudflare.
        let probe = FakeProbe {
            dns: resolving(&[
                "2606:4700:3034::ac43:c93a",
                "2606:4700:3032::6815:551c",
                "91.99.180.122",
            ]),
            open: vec![80, 443],
            web: Some(network(Mode::FullStrict, true)),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        assert!(all_passed(&report.checks), "{:?}", report.checks);
        assert_eq!(
            report.checks[0].detail,
            "A → this server · AAAA → Cloudflare proxy"
        );
        assert_eq!(
            report.checks[1].detail,
            "http://deploy.example.com reaches this server: IPv6 through Cloudflare · IPv4 directly"
        );
        assert_eq!(
            report.cloudflare,
            Some(CloudflareProxy {
                records: vec!["AAAA".into()],
                direct: vec!["A".into()],
                flexible: false
            })
        );
        let seen = probe.seen.lock().unwrap();
        let token = verify_path(NONCE);
        for want in [
            format!("http {token} 91.99.180.122"),
            format!("http {token} 2606:4700:3034::ac43:c93a"),
            format!("https {token} 2606:4700:3034::ac43:c93a"),
            "https /healthz 91.99.180.122".to_string(),
            "https /healthz 2606:4700:3034::ac43:c93a".to_string(),
        ] {
            assert!(seen.contains(&want), "{want} not in {seen:?}");
        }
    }

    #[tokio::test]
    async fn the_flexible_ssl_mode_is_named_with_the_fix() {
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1"]),
            web: Some(network(Mode::Flexible, true)),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        let https = &report.checks[2];
        assert_eq!(https.status, CheckStatus::Fail);
        assert!(https.detail.contains("Flexible"), "{https:?}");
        assert!(https.hint.as_deref().unwrap().contains("Full (strict)"));
        assert!(report.cloudflare.unwrap().flexible);
    }

    #[tokio::test]
    async fn a_redirect_loop_through_an_old_proxy_is_named_too() {
        // A proxy from before the verification route redirects every http request.
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1"]),
            web: Some(Box::new(|req| match req.scheme {
                Scheme::Http => ok(200, &format!("{NONCE} http")),
                Scheme::Https => redirect(format!("https://{}{}", req.domain, req.path)),
            })),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        assert_eq!(report.checks[2].status, CheckStatus::Fail);
        assert!(report.checks[2].detail.contains("redirect loop"));
        assert!(
            report.checks[2]
                .hint
                .as_deref()
                .unwrap()
                .contains("Full (strict)")
        );
    }

    #[tokio::test]
    async fn always_use_https_at_cloudflare_blocks_the_http_challenge() {
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1"]),
            web: Some(Box::new(|req| match req.scheme {
                Scheme::Http => redirect(format!("https://{}{}", req.domain, req.path)),
                Scheme::Https => edge(req, Mode::FullStrict, true),
            })),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        let http = &report.checks[1];
        assert_eq!(http.status, CheckStatus::Fail);
        assert!(http.detail.contains("redirects"), "{http:?}");
        assert!(http.hint.as_deref().unwrap().contains("Always Use HTTPS"));
    }

    #[tokio::test]
    async fn cloudflare_error_pages_say_what_cloudflare_saw() {
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1"]),
            web: Some(Box::new(|_| ok(521, "<html>Web server is down</html>"))),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        let http = &report.checks[1];
        assert_eq!(http.status, CheckStatus::Fail);
        assert!(
            http.detail
                .contains("521: this server refused the connection"),
            "{http:?}"
        );
        assert!(http.hint.as_deref().unwrap().contains("91.99.180.122"));
    }

    #[tokio::test]
    async fn behind_cloudflare_a_pending_origin_certificate_fails_only_the_certificate() {
        let probe = FakeProbe {
            dns: resolving(&["104.21.80.1"]),
            web: Some(network(Mode::FullStrict, false)),
            ..Default::default()
        };
        let report = run(&probe, &["91.99.180.122"]).await;
        assert_eq!(
            statuses(&report.checks),
            [
                CheckStatus::Pass,
                CheckStatus::Pass,
                CheckStatus::Pass,
                CheckStatus::Fail,
                CheckStatus::Skipped
            ],
            "{:?}",
            report.checks
        );
    }

    #[tokio::test]
    async fn a_family_this_server_cannot_route_is_not_tested_instead_of_failing() {
        let probe = FakeProbe {
            dns: resolving(&["203.0.113.4", "2001:db8::4"]),
            open: vec![80, 443],
            web: Some(Box::new(|req| {
                if req.via.is_some_and(|ip| ip.is_ipv6()) {
                    Err(FetchError::NoRoute("Network is unreachable".into()))
                } else {
                    origin(req, req.scheme, true)
                }
            })),
            ..Default::default()
        };
        let report = run(&probe, &["203.0.113.4", "2001:db8::4"]).await;
        assert!(all_passed(&report.checks), "{:?}", report.checks);
        assert!(
            report.checks[1]
                .detail
                .contains("IPv6 not tested, this server has no IPv6 route"),
            "{:?}",
            report.checks[1]
        );
    }
}

/// The checks against the real proxy container, with a stand-in for the daemon's routes.
#[cfg(test)]
mod real_proxy {
    use super::*;
    use crate::docker::{self, InstanceRoute, ProxyConfig};
    use axum::extract::Path;
    use axum::http::HeaderMap;
    use axum::routing::get;
    use std::process::Command;

    const DOMAIN: &str = "cf.localhost";
    const NONCE: &str = "feedc0de";
    const VISITOR: &str = "198.51.100.77";

    /// NetProbe with DNS answered by the test.
    struct Resolving {
        net: NetProbe,
        ips: Vec<IpAddr>,
    }

    impl Probe for Resolving {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>>> {
            Box::pin(async move { Ok(self.ips.clone()) })
        }
        fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, Result<()>> {
            self.net.connect(addr)
        }
        fn get<'a>(&'a self, req: Get<'a>) -> BoxFuture<'a, Result<Reply, FetchError>> {
            self.net.get(req)
        }
        fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)> {
            Box::pin(async { (None, None) })
        }
    }

    /// The daemon's verification route, plus `/ip` echoing the visitor address the proxy forwards.
    async fn stand_in() -> u16 {
        let app = axum::Router::new()
            .route(
                "/.well-known/acme-challenge/{token}",
                get(|Path(token): Path<String>, headers: HeaderMap| async move {
                    let scheme = headers
                        .get(scheme_header())
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    verify_answer(&token, &[NONCE.to_string()], scheme.as_deref())
                        .ok_or(axum::http::StatusCode::NOT_FOUND)
                }),
            )
            .route("/healthz", get(|| async { "ok" }))
            .route(
                "/ip",
                get(|headers: HeaderMap| async move {
                    headers
                        .get("x-real-ip")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                }),
            );
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    fn docker_cli(args: &[&str]) -> String {
        let out = Command::new("docker").args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    }

    async fn install(port: u16, proxied: bool, trusted: &str) {
        let docker = bollard::Docker::connect_with_defaults().unwrap();
        let config = ProxyConfig {
            route: Some(InstanceRoute {
                domain: DOMAIN.into(),
                port,
                proxied,
            }),
            trusted: trusted.into(),
        };
        let pages = crate::proxy_pages::archive(&[]).unwrap();
        docker::install_proxy(&docker, &config, pages)
            .await
            .unwrap();
        assert_eq!(
            docker::proxy_labels_now(&docker).await.unwrap(),
            Some(docker::proxy_container_labels(&config)),
            "the running proxy reads back as up to date"
        );
    }

    /// Waits until the proxy routes the domain to the stand-in.
    async fn ready(probe: &dyn Probe) {
        let path = verify_path(NONCE);
        for _ in 0..60 {
            let req = Get {
                scheme: Scheme::Http,
                domain: DOMAIN,
                path: &path,
                via: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            };
            if probe.get(req).await.is_ok_and(|r| r.status == 200) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!(
            "the proxy never routed {DOMAIN}: {}",
            docker_cli(&["logs", "--tail", "30", brand::PROXY_CONTAINER])
        );
    }

    /// Status, content type and body of an https GET through the proxy.
    async fn fetch(probe: &NetProbe, path: &str, accept: &str) -> Option<(u16, String, String)> {
        let client = NetProbe::builder(&probe.roots)
            .resolve(DOMAIN, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .build()
            .unwrap();
        let res = client
            .get(format!("https://{DOMAIN}{path}"))
            .header("Accept", accept)
            .send()
            .await
            .ok()?;
        let kind = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Some((res.status().as_u16(), kind, res.text().await.ok()?))
    }

    async fn visitor_ip(probe: &NetProbe) -> String {
        let client = NetProbe::builder(&probe.roots)
            .resolve(DOMAIN, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .build()
            .unwrap();
        client
            .get(format!("https://{DOMAIN}/ip"))
            .header("CF-Connecting-IP", VISITOR)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "needs Docker and free ports 80 and 443"]
    async fn the_token_comes_back_through_the_real_proxy() {
        let port = stand_in().await;
        let bundled = Ranges::bundled().to_list();
        install(port, false, &bundled).await;

        let root = {
            let mut pem = String::new();
            for _ in 0..30 {
                pem = docker_cli(&[
                    "exec",
                    brand::PROXY_CONTAINER,
                    "cat",
                    "/data/caddy/pki/authorities/local/root.crt",
                ]);
                if pem.contains("BEGIN CERTIFICATE") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            pem
        };
        let probe = Resolving {
            net: NetProbe::with_root(root.as_bytes()).unwrap(),
            ips: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        };
        ready(&probe).await;

        // Plain http is answered by the daemon, not redirected, and says it arrived over http:
        // exactly what Cloudflare's Flexible mode would produce.
        let path = verify_path(NONCE);
        let plain = probe
            .get(Get {
                scheme: Scheme::Http,
                domain: DOMAIN,
                path: &path,
                via: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            })
            .await
            .unwrap();
        assert_eq!(plain.body, format!("{NONCE} http"));
        let other = probe
            .get(Get {
                scheme: Scheme::Http,
                domain: DOMAIN,
                path: "/healthz",
                via: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            })
            .await
            .unwrap();
        assert_eq!(other.status, 308);
        assert_eq!(
            other.location.as_deref(),
            Some("https://cf.localhost/healthz")
        );

        // The resolver answers with "Cloudflare" addresses: loopback stands in for the edge.
        let edge = Ranges::parse("127.0.0.0/8").unwrap();
        let public = [IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))];
        let report = run_checks(&probe, DOMAIN, &public, &edge, NONCE).await;
        assert!(all_passed(&report.checks), "{:#?}", report.checks);
        assert_eq!(report.checks[0].detail, "proxied by Cloudflare");
        assert_eq!(
            report.cloudflare,
            Some(CloudflareProxy {
                records: vec!["A".into()],
                direct: Vec::new(),
                flexible: false
            })
        );

        // A stale token does not come back.
        let stale = run_checks(&probe, DOMAIN, &public, &edge, "0ld").await;
        assert_eq!(
            stale.checks[1].status,
            CheckStatus::Fail,
            "{:#?}",
            stale.checks
        );
        assert!(
            stale.checks[1]
                .detail
                .contains("without the verification token")
        );

        // Only a trusted proxy's CF-Connecting-IP becomes the visitor address.
        assert_ne!(visitor_ip(&probe.net).await, VISITOR);

        // The daemon's own errors pass through untouched.
        let (status, _, body) = fetch(&probe.net, "/no-such-route", "application/json")
            .await
            .unwrap();
        assert_eq!((status, body.as_str()), (404, ""));

        // With the daemon down, API clients get a JSON error and browsers the branded page.
        let dead = std::net::TcpListener::bind("0.0.0.0:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        install(dead, false, &bundled).await;
        let mut json = None;
        for _ in 0..60 {
            json = fetch(&probe.net, "/healthz", "application/json").await;
            if json.as_ref().is_some_and(|j| j.0 == 503) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let json = json.unwrap();
        assert_eq!(json.0, 503, "{json:?}");
        assert_eq!(json.1, "application/json");
        assert_eq!(
            serde_json::from_str::<common::ApiError>(&json.2)
                .unwrap()
                .error,
            format!("no server is available for {DOMAIN} right now")
        );
        let (status, kind, html) = fetch(&probe.net, "/", "text/html").await.unwrap();
        assert_eq!(status, 503);
        assert!(kind.starts_with("text/html"), "{kind}");
        assert!(html.contains("No server available.") && html.contains(DOMAIN));
        assert!(!html.contains("{{"), "the page is rendered");

        install(
            port,
            false,
            &format!("{bundled} 172.16.0.0/12 192.168.0.0/16 10.0.0.0/8"),
        )
        .await;
        ready(&probe).await;
        assert_eq!(visitor_ip(&probe.net).await, VISITOR);

        // Behind Cloudflare the TLS-ALPN challenge is off for the instance domain.
        install(port, true, &bundled).await;
        let mut config = String::new();
        for _ in 0..30 {
            config = docker_cli(&["logs", brand::PROXY_CONTAINER]);
            if config.contains("disable_tlsalpn_challenge") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert!(
            config.contains(r#"\"tls-alpn\":{\"disabled\":true}"#),
            "{config}"
        );
        assert!(config.contains("Successfully configured"), "{config}");
        docker_cli(&["rm", "-f", brand::PROXY_CONTAINER]);
    }
}
