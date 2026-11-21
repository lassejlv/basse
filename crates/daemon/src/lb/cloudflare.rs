//! The parts of the Cloudflare API the load balancer reconciler uses: DNS records, and the
//! Load Balancing add-on's monitors, pools and load balancers.

use super::RateLimited;
use anyhow::{Context as _, Result, bail};
use common::lb::CloudflareZone;
use reqwest::Method;
use serde_json::{Value, json};
use zeroize::Zeroizing;

pub const DEFAULT_URL: &str = "https://api.cloudflare.com/client/v4";

pub struct Cloudflare {
    http: reqwest::Client,
    base: String,
    token: Zeroizing<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    /// `A`, `AAAA` or `CNAME`.
    pub kind: String,
    pub name: String,
    pub content: String,
    pub proxied: bool,
    pub comment: Option<String>,
}

impl Record {
    /// `A 203.0.113.7`, what the reconciler remembers of a record.
    pub fn value(&self) -> String {
        format!("{} {}", self.kind, self.content)
    }
}

/// One origin of a pool: a server's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub name: String,
    pub address: String,
}

impl Cloudflare {
    pub fn new(http: reqwest::Client, base: &str, token: Zeroizing<String>) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// The `result` of a request. A 429 becomes [`RateLimited`] after `Retry-After` seconds; an
    /// error carries Cloudflare's codes and messages.
    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
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
            .with_context(|| format!("cannot reach the Cloudflare API ({method} {path})"))?;
        let status = res.status();
        if status.as_u16() == 429 {
            let secs = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(60);
            return Err(RateLimited {
                provider: "Cloudflare",
                until: chrono::Utc::now() + chrono::TimeDelta::seconds(secs.max(1)),
            }
            .into());
        }
        let v: Value = res
            .json()
            .await
            .with_context(|| format!("unexpected answer from Cloudflare to {method} {path}"))?;
        if !status.is_success() || v["success"] == Value::Bool(false) {
            let errors: Vec<String> = v["errors"]
                .as_array()
                .map(|es| {
                    es.iter()
                        .map(|e| format!("{} ({})", text(&e["message"]), e["code"]))
                        .collect()
                })
                .unwrap_or_default();
            bail!(
                "Cloudflare answered {status} to {method} {path}: {}",
                errors.join("; ")
            );
        }
        Ok(v)
    }

    async fn pages(&self, path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page = 1;
        loop {
            let sep = if path.contains('?') { '&' } else { '?' };
            let v = self
                .request(
                    Method::GET,
                    &format!("{path}{sep}page={page}&per_page=50"),
                    None,
                )
                .await?;
            out.extend(v["result"].as_array().cloned().unwrap_or_default());
            let total = v["result_info"]["total_pages"].as_i64().unwrap_or(1);
            if page >= total {
                return Ok(out);
            }
            page += 1;
        }
    }

    /// Fails unless the token is active.
    pub async fn verify(&self) -> Result<()> {
        let v = self
            .request(Method::GET, "/user/tokens/verify", None)
            .await?;
        match v["result"]["status"].as_str() {
            Some("active") => Ok(()),
            other => bail!("the token is {}", other.unwrap_or("not active")),
        }
    }

    /// `(id, name)` of every account the token reaches.
    pub async fn accounts(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .pages("/accounts")
            .await?
            .iter()
            .map(|a| (text(&a["id"]), text(&a["name"])))
            .collect())
    }

    /// Whether an account has the Load Balancing add-on: listing its pools is refused without it.
    pub async fn has_load_balancing(&self, account: &str) -> Result<bool> {
        match self
            .request(
                Method::GET,
                &format!("/accounts/{account}/load_balancers/pools"),
                None,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.is::<RateLimited>() => Err(e),
            Err(_) => Ok(false),
        }
    }

    pub async fn zones(&self) -> Result<Vec<CloudflareZone>> {
        Ok(self
            .pages("/zones")
            .await?
            .iter()
            .map(|z| CloudflareZone {
                id: text(&z["id"]),
                name: text(&z["name"]),
                status: text(&z["status"]),
                account_id: text(&z["account"]["id"]),
                account_name: text(&z["account"]["name"]),
            })
            .collect())
    }

    /// The A, AAAA and CNAME records of one name.
    pub async fn records(&self, zone: &str, name: &str) -> Result<Vec<Record>> {
        Ok(self
            .pages(&format!("/zones/{zone}/dns_records?name={name}"))
            .await?
            .iter()
            .map(parse_record)
            .filter(|r| matches!(r.kind.as_str(), "A" | "AAAA" | "CNAME"))
            .collect())
    }

    pub async fn create_record(
        &self,
        zone: &str,
        kind: &str,
        name: &str,
        content: &str,
        proxied: bool,
        comment: &str,
    ) -> Result<Record> {
        let v = self
            .request(
                Method::POST,
                &format!("/zones/{zone}/dns_records"),
                Some(json!({"type": kind, "name": name, "content": content, "proxied": proxied, "ttl": 1, "comment": comment})),
            )
            .await?;
        Ok(parse_record(&v["result"]))
    }

    pub async fn update_record(
        &self,
        zone: &str,
        id: &str,
        kind: &str,
        content: &str,
        proxied: bool,
        comment: &str,
    ) -> Result<()> {
        self.request(
            Method::PATCH,
            &format!("/zones/{zone}/dns_records/{id}"),
            Some(json!({"type": kind, "content": content, "proxied": proxied, "comment": comment})),
        )
        .await?;
        Ok(())
    }

    pub async fn delete_record(&self, zone: &str, id: &str) -> Result<()> {
        self.delete(&format!("/zones/{zone}/dns_records/{id}"))
            .await
    }

    /// An HTTP monitor of the proxy's health endpoint on port 80, sent with the domain as Host.
    pub async fn create_monitor(
        &self,
        account: &str,
        host: &str,
        path: &str,
        description: &str,
    ) -> Result<String> {
        let v = self
            .request(
                Method::POST,
                &format!("/accounts/{account}/load_balancers/monitors"),
                Some(json!({
                    "type": "http",
                    "method": "GET",
                    "path": path,
                    "port": 80,
                    "expected_codes": "2xx",
                    "follow_redirects": false,
                    "header": {"Host": [host]},
                    "interval": 60,
                    "retries": 2,
                    "timeout": 5,
                    "description": description,
                })),
            )
            .await?;
        Ok(text(&v["result"]["id"]))
    }

    pub async fn create_pool(
        &self,
        account: &str,
        name: &str,
        origins: &[Origin],
        monitor: &str,
        description: &str,
    ) -> Result<String> {
        let v = self
            .request(
                Method::POST,
                &format!("/accounts/{account}/load_balancers/pools"),
                Some(json!({
                    "name": name,
                    "description": description,
                    "origins": origins_body(origins),
                    "monitor": monitor,
                    "minimum_origins": 1,
                })),
            )
            .await?;
        Ok(text(&v["result"]["id"]))
    }

    /// A pool's origin addresses, or `None` when it no longer exists.
    pub async fn pool_origins(&self, account: &str, id: &str) -> Result<Option<Vec<Origin>>> {
        match self
            .request(
                Method::GET,
                &format!("/accounts/{account}/load_balancers/pools/{id}"),
                None,
            )
            .await
        {
            Ok(v) => Ok(Some(
                v["result"]["origins"]
                    .as_array()
                    .map(|os| {
                        os.iter()
                            .map(|o| Origin {
                                name: text(&o["name"]),
                                address: text(&o["address"]),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            Err(e) if format!("{e:#}").contains(" 404 ") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn set_pool_origins(
        &self,
        account: &str,
        id: &str,
        origins: &[Origin],
    ) -> Result<()> {
        self.request(
            Method::PATCH,
            &format!("/accounts/{account}/load_balancers/pools/{id}"),
            Some(json!({"origins": origins_body(origins)})),
        )
        .await?;
        Ok(())
    }

    pub async fn create_lb(
        &self,
        zone: &str,
        host: &str,
        pool: &str,
        proxied: bool,
        description: &str,
    ) -> Result<String> {
        let v = self
            .request(
                Method::POST,
                &format!("/zones/{zone}/load_balancers"),
                Some(json!({
                    "name": host,
                    "default_pools": [pool],
                    "fallback_pool": pool,
                    "proxied": proxied,
                    "description": description,
                })),
            )
            .await?;
        Ok(text(&v["result"]["id"]))
    }

    pub async fn lb_exists(&self, zone: &str, id: &str) -> Result<bool> {
        match self
            .request(
                Method::GET,
                &format!("/zones/{zone}/load_balancers/{id}"),
                None,
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if format!("{e:#}").contains(" 404 ") => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub async fn delete_lb(&self, zone: &str, id: &str) -> Result<()> {
        self.delete(&format!("/zones/{zone}/load_balancers/{id}"))
            .await
    }

    pub async fn delete_pool(&self, account: &str, id: &str) -> Result<()> {
        self.delete(&format!("/accounts/{account}/load_balancers/pools/{id}"))
            .await
    }

    pub async fn delete_monitor(&self, account: &str, id: &str) -> Result<()> {
        self.delete(&format!("/accounts/{account}/load_balancers/monitors/{id}"))
            .await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        match self.request(Method::DELETE, path, None).await {
            Ok(_) => Ok(()),
            Err(e) if format!("{e:#}").contains(" 404 ") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The zone a host belongs to: the longest zone name it ends with.
pub fn zone_for<'a>(zones: &'a [CloudflareZone], host: &str) -> Option<&'a CloudflareZone> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    zones
        .iter()
        .filter(|z| {
            let name = z.name.to_ascii_lowercase();
            host == name || host.ends_with(&format!(".{name}"))
        })
        .max_by_key(|z| z.name.len())
}

fn origins_body(origins: &[Origin]) -> Vec<Value> {
    origins
        .iter()
        .map(|o| json!({"name": o.name, "address": o.address, "enabled": true, "weight": 1}))
        .collect()
}

fn parse_record(r: &Value) -> Record {
    Record {
        id: text(&r["id"]),
        kind: text(&r["type"]),
        name: text(&r["name"]),
        content: text(&r["content"]),
        proxied: r["proxied"].as_bool().unwrap_or(false),
        comment: r["comment"].as_str().map(str::to_string),
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(name: &str) -> CloudflareZone {
        CloudflareZone {
            id: name.into(),
            name: name.into(),
            status: "active".into(),
            account_id: "a".into(),
            account_name: "A".into(),
        }
    }

    #[test]
    fn hosts_belong_to_their_longest_zone() {
        let zones = [
            zone("example.com"),
            zone("eu.example.com"),
            zone("other.org"),
        ];
        assert_eq!(
            zone_for(&zones, "api.eu.example.com").unwrap().name,
            "eu.example.com"
        );
        assert_eq!(zone_for(&zones, "example.com").unwrap().name, "example.com");
        assert_eq!(
            zone_for(&zones, "API.Example.com.").unwrap().name,
            "example.com"
        );
        assert!(zone_for(&zones, "notexample.com").is_none());
    }
}
