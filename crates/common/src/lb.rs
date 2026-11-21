//! Provider connections (Hetzner Cloud, Cloudflare) and the managed load balancers the daemon
//! keeps in front of a service's servers.
//!
//! A managed load balancer sends a service's domains to every server that runs a healthy
//! replica of it. The daemon's reconciler keeps the provider's targets in step whenever replicas
//! move, scale, fail their health check or a server goes offline. Everything it creates carries
//! a label (Hetzner) or a comment/description (Cloudflare) naming the instance and service, and
//! it only ever deletes what it created itself. When the provider's side no longer matches what
//! the daemon last applied, someone changed it by hand: the reconciler reports the drift and
//! leaves it alone until a sync is forced.

use crate::{DnsRecord, Id, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A cloud the daemon can talk to with an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Hetzner Cloud: server discovery and Load Balancers.
    Hetzner,
    /// Cloudflare: DNS records, and Load Balancing where the account has it.
    Cloudflare,
}

impl ProviderKind {
    pub const ALL: [Self; 2] = [Self::Hetzner, Self::Cloudflare];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hetzner => "hetzner",
            Self::Cloudflare => "cloudflare",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hetzner => "Hetzner Cloud",
            Self::Cloudflare => "Cloudflare",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "hetzner" | "hcloud" => Ok(Self::Hetzner),
            "cloudflare" | "cf" => Ok(Self::Cloudflare),
            _ => Err(format!(
                "unknown provider '{s}', expected hetzner or cloudflare"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Connected,
    /// The last check failed; `Provider::problem` says why.
    Error,
    #[default]
    Unknown,
}

impl ProviderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

/// A connected provider account. The token is a secret and never returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: Id,
    pub name: String,
    pub kind: ProviderKind,
    /// The `generic` secret holding the API token.
    pub token_secret_id: Id,
    #[serde(default)]
    pub status: ProviderStatus,
    /// Why the last check failed.
    #[serde(default)]
    pub problem: Option<String>,
    /// What the token reaches: the Cloudflare account names, or how many servers the Hetzner
    /// project has, e.g. `4 servers`.
    #[serde(default)]
    pub account: Option<String>,
    /// Cloudflare only: the account has the Load Balancing add-on, so
    /// [`LbMode::CloudflareLb`] is offered.
    #[serde(default)]
    pub load_balancing: bool,
    pub last_checked_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// Body of `POST /providers`. Send either `token`, which is stored as a new `generic` secret,
/// or the id of a secret that already holds it.
#[derive(Clone, Serialize, Deserialize)]
pub struct CreateProvider {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub token_secret_id: Option<Id>,
}

impl fmt::Debug for CreateProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateProvider")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("token", &self.token.as_ref().map(|_| "redacted"))
            .field("token_secret_id", &self.token_secret_id)
            .finish()
    }
}

/// What a provider's token can see, for picking load balancer settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProviderDiscovery {
    /// Hetzner: every server of the daemon, matched to a Hetzner Cloud server of the token's
    /// project by public address.
    #[serde(default)]
    pub servers: Vec<ServerMatch>,
    #[serde(default)]
    pub locations: Vec<HetznerLocation>,
    #[serde(default)]
    pub lb_types: Vec<HetznerLbType>,
    #[serde(default)]
    pub networks: Vec<HetznerNetwork>,
    /// Load balancers in the project, for adopting one.
    #[serde(default)]
    pub load_balancers: Vec<HetznerLbSummary>,
    /// Cloudflare: the zones the token can edit.
    #[serde(default)]
    pub zones: Vec<CloudflareZone>,
}

/// One server of the daemon and what the provider knows about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerMatch {
    pub server_id: Id,
    /// Its public IPv4 address as the daemon resolves it.
    pub address: Option<String>,
    /// The Hetzner Cloud server with that address, when it is in the token's project.
    pub hetzner: Option<HetznerServer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HetznerServer {
    pub id: i64,
    pub name: String,
    /// e.g. `fsn1`.
    pub location: String,
    /// e.g. `eu-central`. A load balancer reaches servers of its own network zone.
    pub network_zone: String,
    #[serde(default)]
    pub public_ipv4: Option<String>,
    #[serde(default)]
    pub public_ipv6: Option<String>,
    /// Private networks the server is attached to, with its address in each.
    #[serde(default)]
    pub private_networks: Vec<PrivateAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateAddress {
    pub network_id: i64,
    pub ip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HetznerLocation {
    pub name: String,
    pub city: String,
    pub network_zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HetznerLbType {
    /// e.g. `lb11`.
    pub name: String,
    pub description: String,
    pub max_targets: u32,
    pub max_connections: u32,
    /// Gross monthly price per location, as Hetzner lists it.
    #[serde(default)]
    pub prices: Vec<LocationPrice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationPrice {
    pub location: String,
    pub monthly: f64,
    /// e.g. `EUR`.
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HetznerNetwork {
    pub id: i64,
    pub name: String,
    pub ip_range: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HetznerLbSummary {
    pub id: i64,
    pub name: String,
    pub location: String,
    pub lb_type: String,
    pub public_ipv4: Option<String>,
    /// Carries this instance's label, so the daemon created it.
    pub managed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareZone {
    pub id: String,
    pub name: String,
    pub status: String,
    pub account_id: String,
    pub account_name: String,
}

/// What kind of managed load balancer a service gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LbMode {
    /// A Hetzner Cloud Load Balancer: TCP passthrough on 80 and 443, so each server's proxy
    /// keeps its certificates, and health checks against the proxy's health endpoint.
    Hetzner,
    /// Free: the domain's proxied A/AAAA records list every server with a healthy replica.
    CloudflareDns,
    /// Cloudflare Load Balancing (paid add-on): a monitor, a pool with one origin per server,
    /// and a load balancer on the domain.
    CloudflareLb,
}

impl LbMode {
    pub const ALL: [Self; 3] = [Self::Hetzner, Self::CloudflareDns, Self::CloudflareLb];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hetzner => "hetzner",
            Self::CloudflareDns => "cloudflare_dns",
            Self::CloudflareLb => "cloudflare_lb",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hetzner => "Hetzner Load Balancer",
            Self::CloudflareDns => "Cloudflare DNS",
            Self::CloudflareLb => "Cloudflare Load Balancing",
        }
    }

    pub fn provider(self) -> ProviderKind {
        match self {
            Self::Hetzner => ProviderKind::Hetzner,
            Self::CloudflareDns | Self::CloudflareLb => ProviderKind::Cloudflare,
        }
    }
}

impl fmt::Display for LbMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LbMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_ascii_lowercase().replace('-', "_");
        match s.as_str() {
            "hetzner" => Ok(Self::Hetzner),
            "cloudflare_dns" | "dns" => Ok(Self::CloudflareDns),
            "cloudflare_lb" | "cloudflare" => Ok(Self::CloudflareLb),
            _ => Err(format!(
                "unknown load balancer mode '{s}', expected hetzner, cloudflare-dns or cloudflare-lb"
            )),
        }
    }
}

/// How a service's managed load balancer is set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbConfig {
    pub provider_id: Id,
    pub mode: LbMode,
    /// The service's domains it handles; empty means all of them.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Hetzner: load balancer type, default `lb11`.
    #[serde(default)]
    pub lb_type: Option<String>,
    /// Hetzner: location, default the location of the first Hetzner server among the targets.
    #[serde(default)]
    pub location: Option<String>,
    /// Hetzner: attach to this private network and reach the servers on their private
    /// addresses. Only allowed when every Hetzner target is in it.
    #[serde(default)]
    pub network_id: Option<i64>,
    /// Hetzner: manage this existing load balancer instead of creating one. It is never
    /// deleted.
    #[serde(default)]
    pub adopt_lb_id: Option<i64>,
    /// Hetzner: the Cloudflare connection whose zone gets the domains' records pointing at
    /// the load balancer. Without one, [`ServiceLb::dns`] lists the records to add by hand.
    #[serde(default)]
    pub dns_provider_id: Option<Id>,
    /// Records the daemon writes go through Cloudflare's proxy (orange cloud). Defaults to
    /// on for the Cloudflare modes and off for a Hetzner load balancer.
    #[serde(default)]
    pub proxied: Option<bool>,
    /// Take over A/AAAA records for the domains that already exist. Without it, enabling
    /// is refused while such records exist. Adopted records are managed like the daemon's own
    /// while it is on, but never deleted when the load balancer is turned off.
    #[serde(default)]
    pub adopt_records: bool,
}

impl LbConfig {
    pub fn proxied(&self) -> bool {
        self.proxied.unwrap_or(self.mode != LbMode::Hetzner)
    }
}

/// Body of `PUT /services/{id}/lb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableLb {
    #[serde(flatten)]
    pub config: LbConfig,
    /// Must be true: the client showed the plan from `POST /services/{id}/lb/plan`.
    #[serde(default)]
    pub confirm: bool,
}

/// What enabling a load balancer would do, and roughly what it costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LbPlan {
    pub actions: Vec<LbAction>,
    /// Targets it would start with.
    pub targets: Vec<LbTarget>,
    /// Rough monthly cost of what gets created; `None` when nothing is billed.
    pub monthly_cost: Option<f64>,
    #[serde(default)]
    pub currency: String,
    /// Where the number comes from, e.g. "Hetzner list price for lb11 in fsn1".
    #[serde(default)]
    pub cost_note: String,
    /// Records to add by hand when no DNS provider is connected.
    #[serde(default)]
    pub dns: Vec<DnsRecord>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbAction {
    pub verb: LbVerb,
    pub resource: LbResourceKind,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LbVerb {
    Create,
    Adopt,
    Update,
    Delete,
}

impl LbVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Adopt => "adopt",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LbResourceKind {
    HetznerLoadBalancer,
    CloudflareDnsRecord,
    CloudflareMonitor,
    CloudflarePool,
    CloudflareLoadBalancer,
}

impl LbResourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::HetznerLoadBalancer => "Hetzner load balancer",
            Self::CloudflareDnsRecord => "DNS record",
            Self::CloudflareMonitor => "Cloudflare monitor",
            Self::CloudflarePool => "Cloudflare pool",
            Self::CloudflareLoadBalancer => "Cloudflare load balancer",
        }
    }
}

/// Something at the provider that belongs to a service's load balancer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbResource {
    pub kind: LbResourceKind,
    /// The provider's id.
    pub id: String,
    pub name: String,
    /// The daemon created it, so it may delete it. Adopted resources are never deleted.
    pub created: bool,
}

/// One server the load balancer sends traffic to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LbTarget {
    pub server_id: Id,
    /// The address the provider is given: public, or private on a Hetzner network.
    pub address: String,
    /// Healthy replicas on the server, as the daemon sees them.
    pub replicas: u16,
    /// What the provider's own health check says: `healthy`, `unhealthy` or `unknown`.
    #[serde(default)]
    pub provider_health: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LbSyncState {
    /// Enabled, not synced yet.
    #[default]
    Pending,
    InSync,
    /// The provider's side was changed outside the daemon; see `ServiceLb::drift`.
    Drift,
    /// The last sync failed; it is retried with backoff.
    Error,
}

impl LbSyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InSync => "in sync",
            Self::Drift => "drift",
            Self::Error => "error",
        }
    }
}

/// A service's managed load balancer and how its last sync went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLb {
    pub config: LbConfig,
    pub state: LbSyncState,
    /// The servers it should send traffic to now, with what the provider reports.
    pub targets: Vec<LbTarget>,
    pub resources: Vec<LbResource>,
    /// The load balancer's public addresses (Hetzner).
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Records to add by hand: Hetzner without a DNS provider.
    #[serde(default)]
    pub dns: Vec<DnsRecord>,
    /// What changed outside the daemon, one line each.
    #[serde(default)]
    pub drift: Vec<String>,
    pub last_sync_at: Option<Timestamp>,
    pub last_error: Option<String>,
    /// When a failed or rate-limited sync is tried again.
    pub next_attempt_at: Option<Timestamp>,
}

/// Result of `DELETE /services/{id}/lb`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LbTeardown {
    pub deleted: Vec<LbResource>,
    /// Left in place: adopted resources always, created ones without `delete_resources`.
    pub kept: Vec<LbResource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_kinds_parse_from_their_cli_spelling() {
        assert_eq!(
            "cloudflare-dns".parse::<LbMode>(),
            Ok(LbMode::CloudflareDns)
        );
        assert_eq!("cloudflare-lb".parse::<LbMode>(), Ok(LbMode::CloudflareLb));
        assert_eq!("hetzner".parse::<ProviderKind>(), Ok(ProviderKind::Hetzner));
        assert!("aws".parse::<ProviderKind>().is_err());
    }

    #[test]
    fn cloudflare_records_are_proxied_unless_said_otherwise() {
        let mut c = LbConfig {
            provider_id: "p".into(),
            mode: LbMode::CloudflareDns,
            domains: vec![],
            lb_type: None,
            location: None,
            network_id: None,
            adopt_lb_id: None,
            dns_provider_id: None,
            proxied: None,
            adopt_records: false,
        };
        assert!(c.proxied());
        c.mode = LbMode::Hetzner;
        assert!(!c.proxied());
        c.proxied = Some(true);
        assert!(c.proxied());
    }

    #[test]
    fn create_provider_never_prints_the_token() {
        let body = CreateProvider {
            name: "hz".into(),
            kind: ProviderKind::Hetzner,
            token: Some("hunter2".into()),
            token_secret_id: None,
        };
        assert!(!format!("{body:?}").contains("hunter2"));
    }
}
