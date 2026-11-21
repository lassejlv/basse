//! Replicas through the real proxy: scaling, a rolling deploy and a killed replica must not
//! fail a single request.

use super::{docker_ok, strings, wait_finished};
use crate::support::Daemon;
use client::Client;
use common::api::{CreateDomain, CreateProject, CreateService, UpdateService};
use common::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HOST: &str = "replicas.localhost";

/// Requests through the proxy from a few workers until stopped.
struct Traffic {
    stop: Arc<AtomicBool>,
    ok: Arc<AtomicUsize>,
    failures: Arc<Mutex<Vec<String>>>,
    hosts: Arc<Mutex<HashSet<String>>>,
    workers: Vec<tokio::task::JoinHandle<()>>,
}

impl Traffic {
    fn start() -> Self {
        let traffic = Self {
            stop: Arc::default(),
            ok: Arc::default(),
            failures: Arc::default(),
            hosts: Arc::default(),
            workers: Vec::new(),
        };
        let workers = (0..3)
            .map(|_| {
                let (stop, ok, failures, hosts) = (
                    traffic.stop.clone(),
                    traffic.ok.clone(),
                    traffic.failures.clone(),
                    traffic.hosts.clone(),
                );
                tokio::spawn(async move {
                    // A fresh connection per request: the proxy closes idle keep-alive
                    // connections whenever it reloads (on every container change), which
                    // browsers retry but reqwest reports as an error.
                    let http = reqwest::Client::builder()
                        .no_proxy()
                        .pool_max_idle_per_host(0)
                        .timeout(Duration::from_secs(10))
                        .build()
                        .unwrap();
                    while !stop.load(Ordering::Relaxed) {
                        let res = http
                            .get("http://127.0.0.1/")
                            .header("Host", HOST)
                            .send()
                            .await;
                        match res {
                            Ok(res) if res.status().is_success() => {
                                let body = res.text().await.unwrap_or_default();
                                if let Some(h) =
                                    body.lines().find_map(|l| l.strip_prefix("Hostname: "))
                                {
                                    hosts.lock().unwrap().insert(h.to_string());
                                }
                                ok.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(res) => failures
                                .lock()
                                .unwrap()
                                .push(format!("HTTP {}", res.status())),
                            Err(e) => {
                                let mut why = e.to_string();
                                let mut source = std::error::Error::source(&e);
                                while let Some(s) = source {
                                    why.push_str(&format!(": {s}"));
                                    source = s.source();
                                }
                                failures.lock().unwrap().push(why);
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
            })
            .collect();
        Self { workers, ..traffic }
    }

    /// Fails the test if any request failed since the last check.
    fn assert_clean(&self, during: &str) {
        let failures = std::mem::take(&mut *self.failures.lock().unwrap());
        assert!(
            failures.is_empty(),
            "{} failed requests {during} ({} ok): {:?}",
            failures.len(),
            self.ok.load(Ordering::Relaxed),
            &failures[..failures.len().min(10)]
        );
    }

    /// Takes the failures since the last check.
    fn take_failures(&self) -> Vec<String> {
        std::mem::take(&mut *self.failures.lock().unwrap())
    }

    fn hosts(&self) -> usize {
        self.hosts.lock().unwrap().len()
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for w in self.workers {
            let _ = w.await;
        }
    }
}

/// Waits until the service runs `n` healthy replicas behind the proxy.
async fn wait_replicas(api: &Client, service_id: &str, n: usize) -> ServiceDetail {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let d = api.service(service_id).await.unwrap();
        if d.replicas.len() == n
            && d.replicas
                .iter()
                .all(|r| r.health == ReplicaHealth::Healthy && r.routed)
        {
            return d;
        }
        assert!(
            Instant::now() < deadline,
            "never reached {n} healthy replicas: {:?}",
            d.replicas
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn route_label(service_id: &str, key: &str) -> Option<String> {
    docker_ok(&strings(&[
        "inspect",
        "--format",
        &format!("{{{{index .Config.Labels {key:?}}}}}"),
        &format!("{}-route-{service_id}", brand::PREFIX),
    ]))
    .ok()
}

/// Removes every container that routes `HOST`, so a run that failed earlier leaves nothing
/// behind: leftover replicas would join the upstreams and a leftover route would disagree
/// with this run's options.
struct Leftovers;

impl Leftovers {
    fn remove() {
        let filter = format!("label=caddy_0=http://{HOST}");
        if let Ok(ids) = docker_ok(&strings(&["ps", "-aq", "--filter", &filter])) {
            for id in ids.lines() {
                let _ = docker_ok(&strings(&["rm", "-f", id]));
            }
        }
    }
}

impl Drop for Leftovers {
    fn drop(&mut self) {
        Self::remove();
    }
}

#[tokio::test]
#[ignore = "needs Docker and free ports 80/443"]
async fn replicas_scale_roll_and_survive_a_killed_replica() {
    let daemon = Daemon::start_on_host()
        .await
        .expect("these tests need Docker");
    Leftovers::remove();
    let _leftovers = Leftovers;
    let api = daemon.client();
    let local = api.servers().await.unwrap().remove(0);
    api.setup_proxy(&local.id).await.unwrap();
    api.create_project(&CreateProject {
        name: "replicas".into(),
        description: None,
    })
    .await
    .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let created = api
        .create_service(&CreateService {
            environment_id: env.id.clone(),
            server_id: None,
            name: "whoami".into(),
            source: Source::Image {
                image: "traefik/whoami".into(),
            },
            port: Some(80),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/health".into()),
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
            replicas: None,
        })
        .await
        .unwrap();
    let sid = created.service.id.clone();
    let first = api.deployments(&sid, 1).await.unwrap().remove(0);
    let first = wait_finished(&api, &first.id, Duration::from_secs(180)).await;
    assert_eq!(first.status, DeploymentStatus::Live, "{first:?}");
    api.add_domain(
        &sid,
        &CreateDomain {
            host: HOST.into(),
            port: None,
            tls: false,
            compose_service: None,
        },
    )
    .await
    .unwrap();
    let config = api.deployments(&sid, 1).await.unwrap().remove(0);
    let config = wait_finished(&api, &config.id, Duration::from_secs(120)).await;
    assert_eq!(config.status, DeploymentStatus::Live, "{config:?}");
    assert_eq!(
        route_label(&sid, "caddy_0.reverse_proxy.lb_policy").as_deref(),
        Some("least_conn")
    );

    let traffic = Traffic::start();
    let deadline = Instant::now() + Duration::from_secs(30);
    while traffic.ok.load(Ordering::Relaxed) == 0 {
        assert!(Instant::now() < deadline, "the proxy never routed {HOST}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    std::mem::take(&mut *traffic.failures.lock().unwrap());

    println!("== scale 1 → 3");
    api.scale(&sid, 3).await.unwrap();
    let detail = wait_replicas(&api, &sid, 3).await;
    assert_eq!(
        detail.replicas.iter().map(|r| r.index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while traffic.hosts() < 3 {
        assert!(
            Instant::now() < deadline,
            "requests never reached all 3 replicas"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    traffic.assert_clean("while scaling up");

    println!("== rolling deploy at 3 replicas");
    let d = api.deploy(&sid, "test").await.unwrap();
    let d = wait_finished(&api, &d.id, Duration::from_secs(300)).await;
    assert_eq!(d.status, DeploymentStatus::Live, "{d:?}");
    let detail = wait_replicas(&api, &sid, 3).await;
    assert!(
        detail
            .replicas
            .iter()
            .all(|r| r.deployment_id.as_deref() == Some(d.id.as_str())),
        "{:?}",
        detail.replicas
    );
    traffic.assert_clean("during the rolling deploy");

    println!("== kill replica #1");
    let victim = detail.replicas[1].container.as_ref().unwrap().id.clone();
    docker_ok(&strings(&["kill", &victim])).unwrap();
    // Requests in flight on the replica as it died are lost: its network goes away without
    // closing their connections, so they run into the client's 10 s timeout, and the proxy
    // reloads on the death, which ends retries in progress. After that, traffic must flow
    // on the other two without a single failure.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let lost = traffic.take_failures();
    assert!(
        lost.len() <= 5,
        "the killed replica cost {} requests: {lost:?}",
        lost.len()
    );
    println!("requests lost to the kill: {}", lost.len());
    tokio::time::sleep(Duration::from_secs(5)).await;
    traffic.assert_clean("after a replica was killed");

    println!("== scale 3 → 1");
    api.scale(&sid, 1).await.unwrap();
    wait_replicas(&api, &sid, 1).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    traffic.assert_clean("while scaling down");

    println!("== load balancing policy");
    api.update_service(
        &sid,
        &UpdateService {
            lb_policy: Some(LbPolicy::RoundRobin),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        route_label(&sid, "caddy_0.reverse_proxy.lb_policy").as_deref(),
        Some("round_robin")
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    traffic.assert_clean("while the route changed");
    let served = traffic.ok.load(Ordering::Relaxed);
    traffic.stop().await;
    assert!(served > 200, "only {served} requests went through");

    println!("== stopped: the route alone answers 503");
    api.stop(&sid).await.unwrap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = http
            .get("http://127.0.0.1/")
            .header("Host", HOST)
            .send()
            .await
            .map(|r| r.status().as_u16())
            .ok();
        if status == Some(503) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stopped service answered {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    api.delete_service(&sid, false).await.unwrap();
    assert_eq!(
        route_label(&sid, "caddy_0"),
        None,
        "the route outlived the service"
    );
}
