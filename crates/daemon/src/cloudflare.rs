//! Cloudflare's published edge address ranges: a domain resolving into them is proxied by
//! Cloudflare (orange cloud), and the proxy trusts the visitor address Cloudflare forwards from
//! them.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const IPS_V4: &str = "https://www.cloudflare.com/ips-v4";
const IPS_V6: &str = "https://www.cloudflare.com/ips-v6";
/// How long fetched ranges are used before they are fetched again.
const REFRESH: Duration = Duration::from_secs(24 * 3600);
/// How long to wait before trying again after a failed fetch.
const RETRY: Duration = Duration::from_secs(3600);

/// The published lists as of 2026-11, used until a fetch succeeds.
const BUNDLED: &str = "173.245.48.0/20
103.21.244.0/22
103.22.200.0/22
103.31.4.0/22
141.101.64.0/18
108.162.192.0/18
190.93.240.0/20
188.114.96.0/20
197.234.240.0/22
198.41.128.0/17
162.158.0.0/15
104.16.0.0/13
104.24.0.0/14
172.64.0.0/13
131.0.72.0/22
2400:cb00::/32
2606:4700::/32
2803:f800::/32
2405:b500::/32
2405:8100::/32
2a06:98c0::/29
2c0f:f248::/32";

/// One CIDR block such as `104.16.0.0/13`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    len: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Self> {
        let (net, len) = s.trim().split_once('/')?;
        let net: IpAddr = net.parse().ok()?;
        let len: u8 = len.parse().ok()?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        (len <= max).then_some(Self { net, len })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        match (self.net, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.len))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.net, self.len)
    }
}

/// A set of ranges, IPv4 first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranges(Vec<Cidr>);

impl Ranges {
    pub fn bundled() -> Self {
        Self::parse(BUNDLED).expect("the bundled ranges parse")
    }

    /// One CIDR per line; `None` when a line is not a CIDR or there are none.
    pub fn parse(text: &str) -> Option<Self> {
        let cidrs = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(Cidr::parse)
            .collect::<Option<Vec<_>>>()?;
        (!cidrs.is_empty()).then_some(Self(cidrs))
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|c| c.contains(ip))
    }

    /// The ranges as `a/b c/d …`, the form the proxy's `trusted_proxies static` takes.
    pub fn to_list(&self) -> String {
        self.0
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The current ranges: fetched from Cloudflare once a day, the bundled list until then.
pub struct CloudflareIps {
    current: Mutex<(Option<Instant>, Ranges)>,
    refreshing: tokio::sync::Mutex<()>,
}

impl Default for CloudflareIps {
    fn default() -> Self {
        Self {
            current: Mutex::new((None, Ranges::bundled())),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }
}

impl CloudflareIps {
    pub async fn get(&self, http: &reqwest::Client) -> Ranges {
        if let Some(ranges) = self.fresh() {
            return ranges;
        }
        let _guard = self.refreshing.lock().await;
        if let Some(ranges) = self.fresh() {
            return ranges;
        }
        let fetched = async {
            let mut text = String::new();
            for url in [IPS_V4, IPS_V6] {
                let res = http.get(url).send().await?.error_for_status()?;
                text.push_str(&res.text().await?);
                text.push('\n');
            }
            anyhow::Ok(text)
        }
        .await;
        let mut current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        match fetched.map(|t| Ranges::parse(&t)) {
            Ok(Some(ranges)) => *current = (Some(Instant::now()), ranges),
            Ok(None) => {
                tracing::warn!("Cloudflare's address lists did not parse; keeping the last ones");
                current.0 = Instant::now().checked_sub(REFRESH - RETRY);
            }
            Err(e) => {
                tracing::warn!("cannot fetch Cloudflare's address lists: {e:#}");
                current.0 = Instant::now().checked_sub(REFRESH - RETRY);
            }
        }
        current.1.clone()
    }

    fn fresh(&self) -> Option<Ranges> {
        let current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        current
            .0
            .filter(|at| at.elapsed() < REFRESH)
            .map(|_| current.1.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn cidrs_match_their_whole_block_and_nothing_else() {
        let c = Cidr::parse("104.16.0.0/13").unwrap();
        assert!(c.contains(ip("104.16.0.0")));
        assert!(c.contains(ip("104.23.255.255")));
        assert!(!c.contains(ip("104.24.0.0")));
        assert!(!c.contains(ip("104.15.255.255")));
        assert!(!c.contains(ip("2606:4700::1")));

        let c = Cidr::parse("2a06:98c0::/29").unwrap();
        assert!(c.contains(ip("2a06:98c7:ffff::1")));
        assert!(!c.contains(ip("2a06:98c8::")));

        assert!(
            Cidr::parse("0.0.0.0/0")
                .unwrap()
                .contains(ip("203.0.113.4"))
        );
        assert!(
            Cidr::parse("203.0.113.4/32")
                .unwrap()
                .contains(ip("203.0.113.4"))
        );
        assert!(
            !Cidr::parse("203.0.113.4/32")
                .unwrap()
                .contains(ip("203.0.113.5"))
        );
    }

    #[test]
    fn malformed_cidrs_are_rejected() {
        for bad in [
            "104.16.0.0",
            "104.16.0.0/33",
            "2606:4700::/129",
            "x/8",
            "1.2.3.4/-1",
        ] {
            assert!(Cidr::parse(bad).is_none(), "{bad}");
        }
        assert!(Ranges::parse("").is_none());
        assert!(Ranges::parse("<html>").is_none());
    }

    #[test]
    fn the_bundled_list_covers_cloudflare_edges() {
        let r = Ranges::bundled();
        for cf in [
            "104.21.80.1",
            "172.67.201.58",
            "188.114.97.3",
            "2606:4700:3034::ac43:c93a",
            "2606:4700:3032::6815:551c",
            "::ffff:104.21.80.1",
        ] {
            assert!(r.contains(ip(cf)), "{cf}");
        }
        for other in [
            "91.99.180.122",
            "203.0.113.4",
            "2a01:4f8:c012:3f6a::1",
            "127.0.0.1",
        ] {
            assert!(!r.contains(ip(other)), "{other}");
        }
        assert!(r.to_list().starts_with("173.245.48.0/20 103.21.244.0/22"));
        assert!(r.to_list().ends_with("2c0f:f248::/32"));
    }
}
