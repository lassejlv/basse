//! The instance domain: the hostname the proxy serves the daemon's own API on with automatic
//! HTTPS, and the checks that guide an operator through pointing DNS at the server.

use anyhow::{Context as _, Result};
use common::{CheckStatus, DnsRecord, DomainCheck, DomainCheckKind, InstanceDomain, brand};
use futures_util::future::BoxFuture;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Services that echo the caller's public address, tried in order.
const IPV4_ECHO: &str = "https://api.ipify.org";
const IPV6_ECHO: &str = "https://api6.ipify.org";

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

/// Where the checks reach out to the world, so tests can stand in for DNS and the network.
pub trait Probe: Send + Sync {
    /// The addresses `host` resolves to.
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>>>;
    /// Opens and closes a TCP connection.
    fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, Result<()>>;
    /// `GET https://<domain><path>` with certificate verification.
    fn https_get<'a>(
        &'a self,
        domain: &'a str,
        path: &'a str,
    ) -> BoxFuture<'a, Result<HttpsReply, HttpsError>>;
    /// This server's public IPv4 and IPv6 address as the internet sees it.
    fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsReply {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpsError {
    /// The TLS handshake failed: no certificate yet, or one that does not match.
    Tls(String),
    Other(String),
}

/// The real network.
pub struct NetProbe {
    http: reqwest::Client,
}

impl NetProbe {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(PROBE_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
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
            let mut ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
            ips.dedup();
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

    fn https_get<'a>(
        &'a self,
        domain: &'a str,
        path: &'a str,
    ) -> BoxFuture<'a, Result<HttpsReply, HttpsError>> {
        Box::pin(async move {
            let res = self
                .http
                .get(format!("https://{domain}{path}"))
                .send()
                .await
                .map_err(|e| {
                    let chain = error_chain(&e);
                    let lower = chain.to_ascii_lowercase();
                    if ["certificate", "tls", "handshake", "alert"]
                        .iter()
                        .any(|w| lower.contains(w))
                    {
                        HttpsError::Tls(chain)
                    } else {
                        HttpsError::Other(chain)
                    }
                })?;
            let status = res.status().as_u16();
            let body = res.text().await.unwrap_or_default();
            Ok(HttpsReply { status, body })
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

/// Runs the five checks in order. A check whose prerequisites failed is skipped.
pub async fn run_checks(probe: &dyn Probe, domain: &str, public: &[IpAddr]) -> Vec<DomainCheck> {
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
            return checks;
        }
    };
    let shown = resolved
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let target = resolved.iter().copied().find(|ip| public.contains(ip));
    match target {
        Some(_) => checks.push(check(Dns, CheckStatus::Pass, format!("resolves to {shown}"), None)),
        None if public.is_empty() => checks.push(check(
            Dns,
            CheckStatus::Pass,
            format!("resolves to {shown}; this server's public address is unknown, so it was not compared"),
            None,
        )),
        None => {
            checks.push(check(
                Dns,
                CheckStatus::Fail,
                format!("resolves to {shown}, not this server ({public_list})"),
                Some(format!(
                    "change the record to {public_list}; behind a CDN proxy such as Cloudflare, set the record to DNS only"
                )),
            ));
            checks.extend([Http, Https, Certificate, Api].map(skipped));
            return checks;
        }
    }
    let ip = target.unwrap_or(resolved[0]);

    for (kind, port) in [(Http, 80u16), (Https, 443)] {
        checks.push(match probe.connect(SocketAddr::new(ip, port)).await {
            Ok(()) => check(kind, CheckStatus::Pass, format!("{ip}:{port} accepts connections"), None),
            Err(e) => check(
                kind,
                CheckStatus::Fail,
                format!("cannot connect to {ip}:{port}: {e:#}"),
                Some(format!(
                    "open port {port}/tcp in the server's and the provider's firewall (ufw allow {port}/tcp) and make sure the proxy runs; some providers block a server from reaching its own public address, so also try curl -I http://{domain} from another machine"
                )),
            ),
        });
    }
    if !(passed(&checks, Http) && passed(&checks, Https)) {
        checks.extend([Certificate, Api].map(skipped));
        return checks;
    }

    match probe.https_get(domain, "/healthz").await {
        Err(HttpsError::Tls(e)) => {
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
        }
        Err(HttpsError::Other(e)) => {
            checks.push(check(
                Certificate,
                CheckStatus::Fail,
                format!("https://{domain} did not answer: {e}"),
                Some("check again in a minute; the proxy may still be starting".into()),
            ));
            checks.push(skipped(Api));
        }
        Ok(reply) => {
            checks.push(check(
                Certificate,
                CheckStatus::Pass,
                format!("https://{domain} serves a valid certificate"),
                None,
            ));
            checks.push(if reply.status == 200 && reply.body.trim() == "ok" {
                check(Api, CheckStatus::Pass, format!("https://{domain} reaches the API"), None)
            } else {
                check(
                    Api,
                    CheckStatus::Fail,
                    format!("https://{domain}/healthz answered {}", reply.status),
                    Some(format!(
                        "reinstall the proxy so it routes the domain to the API: {} instance domain set {domain}",
                        brand::CLI_BIN
                    )),
                )
            });
        }
    }
    checks
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
    last: Option<(Vec<DomainCheck>, common::Timestamp)>,
) -> InstanceDomain {
    let Some(domain) = domain else {
        return InstanceDomain {
            public_ipv4: v4.map(|ip| ip.to_string()),
            public_ipv6: v6.map(|ip| ip.to_string()),
            ..Default::default()
        };
    };
    let (checks, checked_at) = match last {
        Some((checks, at)) => (checks, Some(at)),
        None => (Vec::new(), None),
    };
    InstanceDomain {
        domain: Some(domain.to_string()),
        url: verified_at.map(|_| format!("https://{domain}")),
        verified: verified_at.is_some(),
        verified_at,
        public_ipv4: v4.map(|ip| ip.to_string()),
        public_ipv6: v6.map(|ip| ip.to_string()),
        records: records(domain, v4, v6),
        checks,
        checked_at,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A network where DNS answers, open ports and HTTPS replies are set by the test.
    #[derive(Default)]
    pub struct FakeProbe {
        pub dns: HashMap<String, Vec<IpAddr>>,
        pub open: Vec<u16>,
        pub https: Mutex<Option<Result<HttpsReply, HttpsError>>>,
        pub public: (Option<IpAddr>, Option<IpAddr>),
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

        fn https_get<'a>(
            &'a self,
            _domain: &'a str,
            _path: &'a str,
        ) -> BoxFuture<'a, Result<HttpsReply, HttpsError>> {
            Box::pin(async move {
                self.https
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or(Err(HttpsError::Other("unreachable".into())))
            })
        }

        fn public_ips(&self) -> BoxFuture<'_, (Option<IpAddr>, Option<IpAddr>)> {
            Box::pin(async move { self.public })
        }
    }

    const D: &str = "deploy.example.com";

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn statuses(checks: &[DomainCheck]) -> Vec<CheckStatus> {
        checks.iter().map(|c| c.status).collect()
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
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert_eq!(
            statuses(&checks),
            [
                CheckStatus::Fail,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
                CheckStatus::Skipped,
                CheckStatus::Skipped
            ]
        );
        assert!(checks[0].hint.as_deref().unwrap().contains("203.0.113.4"));
    }

    #[tokio::test]
    async fn a_record_pointing_elsewhere_names_both_addresses() {
        let probe = FakeProbe {
            dns: HashMap::from([(D.to_string(), vec![ip("198.51.100.9")])]),
            ..Default::default()
        };
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert_eq!(checks[0].status, CheckStatus::Fail);
        assert!(checks[0].detail.contains("198.51.100.9"));
        assert!(checks[0].detail.contains("203.0.113.4"));
    }

    #[tokio::test]
    async fn closed_ports_fail_individually() {
        let probe = FakeProbe {
            dns: HashMap::from([(D.to_string(), vec![ip("203.0.113.4")])]),
            open: vec![80],
            ..Default::default()
        };
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert_eq!(
            statuses(&checks),
            [
                CheckStatus::Pass,
                CheckStatus::Pass,
                CheckStatus::Fail,
                CheckStatus::Skipped,
                CheckStatus::Skipped
            ]
        );
        assert!(checks[2].hint.as_deref().unwrap().contains("443"));
    }

    #[tokio::test]
    async fn a_missing_certificate_is_reported_with_the_tls_error() {
        let probe = FakeProbe {
            dns: HashMap::from([(D.to_string(), vec![ip("203.0.113.4")])]),
            open: vec![80, 443],
            https: Mutex::new(Some(Err(HttpsError::Tls(
                "tlsv1 alert internal error".into(),
            )))),
            ..Default::default()
        };
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert_eq!(checks[3].status, CheckStatus::Fail);
        assert!(checks[3].detail.contains("alert"));
        assert_eq!(checks[4].status, CheckStatus::Skipped);
        assert!(!all_passed(&checks));
    }

    #[tokio::test]
    async fn everything_passing_verifies_the_domain() {
        let probe = FakeProbe {
            dns: HashMap::from([(D.to_string(), vec![ip("2001:db8::4"), ip("203.0.113.4")])]),
            open: vec![80, 443],
            https: Mutex::new(Some(Ok(HttpsReply {
                status: 200,
                body: "ok".into(),
            }))),
            ..Default::default()
        };
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert!(all_passed(&checks), "{checks:?}");
        let s = status(
            Some(D),
            Some(chrono::Utc::now()),
            Some(ip("203.0.113.4")),
            None,
            Some((checks, chrono::Utc::now())),
        );
        assert_eq!(s.url.as_deref(), Some("https://deploy.example.com"));
    }

    #[tokio::test]
    async fn a_proxy_that_does_not_route_to_the_api_fails_the_last_check() {
        let probe = FakeProbe {
            dns: HashMap::from([(D.to_string(), vec![ip("203.0.113.4")])]),
            open: vec![80, 443],
            https: Mutex::new(Some(Ok(HttpsReply {
                status: 502,
                body: String::new(),
            }))),
            ..Default::default()
        };
        let checks = run_checks(&probe, D, &[ip("203.0.113.4")]).await;
        assert_eq!(checks[3].status, CheckStatus::Pass);
        assert_eq!(checks[4].status, CheckStatus::Fail);
        assert!(checks[4].detail.contains("502"));
    }
}
