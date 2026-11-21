//! The parts of the Hetzner Cloud API the load balancer reconciler uses.
//!
//! Everything goes through [`Hetzner::request`], which turns a 429 into [`RateLimited`] with the
//! time `RateLimit-Reset` names, and any other error into Hetzner's own message.

use super::RateLimited;
use anyhow::{Context as _, Result, bail};
use common::lb::{
    HetznerLbSummary, HetznerLbType, HetznerLocation, HetznerNetwork, HetznerServer, LocationPrice,
    PrivateAddress,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use zeroize::Zeroizing;

pub const DEFAULT_URL: &str = "https://api.hetzner.cloud/v1";

pub struct Hetzner {
    http: reqwest::Client,
    base: String,
    token: Zeroizing<String>,
}

/// Where a load balancer sends traffic. Hetzner Cloud servers of the same project are added by
/// id; other servers by public address, which Hetzner only accepts for its dedicated servers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Target {
    Server { id: i64, private: bool },
    Ip(String),
}

impl Target {
    /// Stable text form, kept in what the reconciler last applied.
    pub fn key(&self) -> String {
        match self {
            Self::Server { id, private: false } => format!("server:{id}"),
            Self::Server { id, private: true } => format!("server:{id}:private"),
            Self::Ip(ip) => format!("ip:{ip}"),
        }
    }

    fn body(&self) -> Value {
        match self {
            Self::Server { id, private } => {
                json!({"type": "server", "server": {"id": id}, "use_private_ip": private})
            }
            Self::Ip(ip) => json!({"type": "ip", "ip": {"ip": ip}}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadBalancer {
    pub id: i64,
    pub name: String,
    pub location: String,
    pub lb_type: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub labels: HashMap<String, String>,
    /// Each target with its health on port 80: `healthy`, `unhealthy` or `unknown`.
    pub targets: Vec<(Target, Option<String>)>,
    pub listen_ports: Vec<u16>,
}

/// A new load balancer: TCP passthrough on 80 and 443 with health checks on port 80.
pub struct NewLb<'a> {
    pub name: &'a str,
    pub lb_type: &'a str,
    pub location: &'a str,
    pub network_id: Option<i64>,
    pub labels: HashMap<String, String>,
    pub health_host: &'a str,
    pub health_path: &'a str,
    pub targets: &'a [Target],
}

impl Hetzner {
    pub fn new(http: reqwest::Client, base: &str, token: Zeroizing<String>) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(self.token.as_str());
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
            .send()
            .await
            .with_context(|| format!("cannot reach the Hetzner API ({method} {path})"))?;
        let status = res.status();
        if status.as_u16() == 429 {
            let until = res
                .headers()
                .get("ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                .unwrap_or_else(|| chrono::Utc::now() + chrono::TimeDelta::seconds(60));
            return Err(RateLimited {
                provider: "Hetzner",
                until,
            }
            .into());
        }
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            bail!("Hetzner answered {status} to {method} {path}: {message}");
        }
        if text.is_empty() {
            return Ok(serde_json::from_value(Value::Null)?);
        }
        serde_json::from_str(&text)
            .with_context(|| format!("unexpected answer from Hetzner to {method} {path}"))
    }

    async fn pages(&self, path: &str, key: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let sep = if path.contains('?') { '&' } else { '?' };
            let v: Value = self
                .request(
                    Method::GET,
                    &format!("{path}{sep}page={page}&per_page=50"),
                    None,
                )
                .await?;
            out.extend(v[key].as_array().cloned().unwrap_or_default());
            match v["meta"]["pagination"]["next_page"].as_i64() {
                Some(next) if next > page => page = next,
                _ => return Ok(out),
            }
        }
    }

    /// Every server of the token's project.
    pub async fn servers(&self) -> Result<Vec<HetznerServer>> {
        Ok(self
            .pages("/servers", "servers")
            .await?
            .iter()
            .map(|s| HetznerServer {
                id: s["id"].as_i64().unwrap_or_default(),
                name: text(&s["name"]),
                location: text(&s["datacenter"]["location"]["name"]),
                network_zone: text(&s["datacenter"]["location"]["network_zone"]),
                public_ipv4: s["public_net"]["ipv4"]["ip"].as_str().map(str::to_string),
                public_ipv6: s["public_net"]["ipv6"]["ip"].as_str().map(str::to_string),
                private_networks: s["private_net"]
                    .as_array()
                    .map(|nets| {
                        nets.iter()
                            .map(|n| PrivateAddress {
                                network_id: n["network"].as_i64().unwrap_or_default(),
                                ip: text(&n["ip"]),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    pub async fn locations(&self) -> Result<Vec<HetznerLocation>> {
        Ok(self
            .pages("/locations", "locations")
            .await?
            .iter()
            .map(|l| HetznerLocation {
                name: text(&l["name"]),
                city: text(&l["city"]),
                network_zone: text(&l["network_zone"]),
            })
            .collect())
    }

    pub async fn lb_types(&self) -> Result<Vec<HetznerLbType>> {
        Ok(self
            .pages("/load_balancer_types", "load_balancer_types")
            .await?
            .iter()
            .map(|t| HetznerLbType {
                name: text(&t["name"]),
                description: text(&t["description"]),
                max_targets: t["max_targets"].as_u64().unwrap_or_default() as u32,
                max_connections: t["max_connections"].as_u64().unwrap_or_default() as u32,
                prices: t["prices"]
                    .as_array()
                    .map(|ps| {
                        ps.iter()
                            .map(|p| LocationPrice {
                                location: text(&p["location"]),
                                monthly: p["price_monthly"]["gross"]
                                    .as_str()
                                    .and_then(|g| g.parse().ok())
                                    .unwrap_or_default(),
                                currency: "EUR".into(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    pub async fn networks(&self) -> Result<Vec<HetznerNetwork>> {
        Ok(self
            .pages("/networks", "networks")
            .await?
            .iter()
            .map(|n| HetznerNetwork {
                id: n["id"].as_i64().unwrap_or_default(),
                name: text(&n["name"]),
                ip_range: text(&n["ip_range"]),
            })
            .collect())
    }

    pub async fn load_balancers(&self) -> Result<Vec<LoadBalancer>> {
        Ok(self
            .pages("/load_balancers", "load_balancers")
            .await?
            .iter()
            .map(parse_lb)
            .collect())
    }

    /// The load balancer, or `None` when it no longer exists.
    pub async fn load_balancer(&self, id: i64) -> Result<Option<LoadBalancer>> {
        match self
            .request::<Value>(Method::GET, &format!("/load_balancers/{id}"), None)
            .await
        {
            Ok(v) => Ok(Some(parse_lb(&v["load_balancer"]))),
            Err(e) if format!("{e:#}").contains(" 404 ") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn create_lb(&self, lb: &NewLb<'_>) -> Result<LoadBalancer> {
        let services: Vec<Value> = [80, 443]
            .iter()
            .map(|port| service_body(*port, lb.health_host, lb.health_path))
            .collect();
        let targets: Vec<Value> = lb.targets.iter().map(Target::body).collect();
        let mut body = json!({
            "name": lb.name,
            "load_balancer_type": lb.lb_type,
            "location": lb.location,
            "labels": lb.labels,
            "algorithm": {"type": "least_connections"},
            "public_interface": true,
            "services": services,
            "targets": targets,
        });
        if let Some(network) = lb.network_id {
            body["network"] = json!(network);
        }
        let v: Value = self
            .request(Method::POST, "/load_balancers", Some(body))
            .await?;
        Ok(parse_lb(&v["load_balancer"]))
    }

    /// Adds the 80 and 443 services an adopted load balancer lacks.
    pub async fn add_service(&self, id: i64, port: u16, host: &str, path: &str) -> Result<()> {
        self.request::<Value>(
            Method::POST,
            &format!("/load_balancers/{id}/actions/add_service"),
            Some(service_body(port, host, path)),
        )
        .await?;
        Ok(())
    }

    pub async fn add_target(&self, id: i64, target: &Target) -> Result<()> {
        self.request::<Value>(
            Method::POST,
            &format!("/load_balancers/{id}/actions/add_target"),
            Some(target.body()),
        )
        .await?;
        Ok(())
    }

    pub async fn remove_target(&self, id: i64, target: &Target) -> Result<()> {
        let mut body = target.body();
        if let Some(obj) = body.as_object_mut() {
            obj.remove("use_private_ip");
        }
        self.request::<Value>(
            Method::POST,
            &format!("/load_balancers/{id}/actions/remove_target"),
            Some(body),
        )
        .await?;
        Ok(())
    }

    pub async fn delete_lb(&self, id: i64) -> Result<()> {
        match self
            .request::<Value>(Method::DELETE, &format!("/load_balancers/{id}"), None)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if format!("{e:#}").contains(" 404 ") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// TCP passthrough: TLS ends on each server's proxy, which keeps its certificates. The health
/// check asks the proxy's health endpoint on port 80 for the domain, which answers with the
/// service's health path on that server's replicas.
fn service_body(port: u16, host: &str, path: &str) -> Value {
    json!({
        "protocol": "tcp",
        "listen_port": port,
        "destination_port": port,
        "proxyprotocol": false,
        "health_check": {
            "protocol": "http",
            "port": 80,
            "interval": 15,
            "timeout": 10,
            "retries": 3,
            "http": {
                "domain": host,
                "path": path,
                "status_codes": ["2??", "3??"],
                "tls": false
            }
        }
    })
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn parse_lb(v: &Value) -> LoadBalancer {
    let targets = v["targets"]
        .as_array()
        .map(|ts| {
            ts.iter()
                .filter_map(|t| {
                    let target = match t["type"].as_str()? {
                        "server" => Target::Server {
                            id: t["server"]["id"].as_i64()?,
                            private: t["use_private_ip"].as_bool().unwrap_or(false),
                        },
                        "ip" => Target::Ip(t["ip"]["ip"].as_str()?.to_string()),
                        _ => return None,
                    };
                    let health = t["health_status"].as_array().and_then(|hs| {
                        hs.iter()
                            .find(|h| h["listen_port"].as_u64() == Some(80))
                            .or_else(|| hs.first())
                            .and_then(|h| h["status"].as_str().map(str::to_string))
                    });
                    Some((target, health))
                })
                .collect()
        })
        .unwrap_or_default();
    LoadBalancer {
        id: v["id"].as_i64().unwrap_or_default(),
        name: text(&v["name"]),
        location: text(&v["location"]["name"]),
        lb_type: text(&v["load_balancer_type"]["name"]),
        ipv4: v["public_net"]["ipv4"]["ip"].as_str().map(str::to_string),
        ipv6: v["public_net"]["ipv6"]["ip"].as_str().map(str::to_string),
        labels: serde_json::from_value(v["labels"].clone()).unwrap_or_default(),
        targets,
        listen_ports: v["services"]
            .as_array()
            .map(|s| {
                s.iter()
                    .filter_map(|s| s["listen_port"].as_u64())
                    .map(|p| p as u16)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl LoadBalancer {
    pub fn summary(&self, managed: bool) -> HetznerLbSummary {
        HetznerLbSummary {
            id: self.id,
            name: self.name.clone(),
            location: self.location.clone(),
            lb_type: self.lb_type.clone(),
            public_ipv4: self.ipv4.clone(),
            managed,
        }
    }
}
