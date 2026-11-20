//! API contract tests against the real binary. They need no Docker daemon.

mod support;

use common::api::*;
use common::*;
use reqwest::StatusCode;
use support::Daemon;

#[tokio::test]
async fn auth_is_required_and_errors_are_json() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let http = reqwest::Client::new();
    let url = format!("{}{API_PREFIX}/projects", daemon.url);

    let res = http.get(&url).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body: ApiError = res.json().await.unwrap();
    assert_eq!(body.error, "unauthorized");

    let res = http.get(&url).bearer_auth("wrong").send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = http
        .post(&url)
        .bearer_auth(&daemon.token)
        .body("{nope")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let _: ApiError = res.json().await.unwrap();

    let res = http
        .get(format!("{}{API_PREFIX}/nope", daemon.url))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let _: ApiError = res.json().await.unwrap();

    let res = http
        .get(format!("{}{API_PREFIX}/deployments/missing", daemon.url))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = http
        .get(format!("{}/healthz", daemon.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn projects_environments_and_services_crud() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let api = daemon.client();
    let http = reqwest::Client::new();

    let servers = api.servers().await.unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].kind, ServerKind::Local);
    assert_eq!(servers[0].name, "local");

    let project = api
        .create_project(&CreateProject {
            name: "shop".into(),
            description: Some("store".into()),
        })
        .await
        .unwrap();
    let res = http
        .post(format!("{}{API_PREFIX}/projects", daemon.url))
        .bearer_auth(&daemon.token)
        .json(&CreateProject {
            name: "shop".into(),
            description: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let err = api
        .create_project(&CreateProject {
            name: "Bad Name".into(),
            description: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid name"), "{err}");

    let staging = api
        .create_environment(
            &project.id,
            &CreateEnvironment {
                name: "staging".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        api.create_environment(
            &project.id,
            &CreateEnvironment {
                name: "staging".into()
            }
        )
        .await
        .is_err()
    );
    let tree = api.tree().await.unwrap();
    let node = &tree.projects[0];
    let env_names: Vec<_> = node
        .environments
        .iter()
        .map(|e| e.environment.name.as_str())
        .collect();
    assert_eq!(env_names, ["production", "staging"]);
    let production = node.environments[0].environment.clone();

    let web = api
        .create_service(&CreateService {
            environment_id: production.id.clone(),
            server_id: None,
            name: "web".into(),
            source: Source::Image {
                image: "nginx:alpine".into(),
            },
            port: Some(80),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: Some("/".into()),
            env: vec![EnvVar {
                key: "A".into(),
                value: "1".into(),
                secret: false,
            }],
            domains: vec!["Shop.Example.com".into()],
            volumes: vec![VolumeSpec {
                name: "cache".into(),
                mount_path: "/cache".into(),
            }],
            deploy: false,
        })
        .await
        .unwrap();
    assert_eq!(web.service.state, ServiceState::New);
    assert_eq!(
        web.service.primary_domain.as_deref(),
        Some("shop.example.com")
    );
    assert_eq!(web.server.kind, ServerKind::Local);
    assert_eq!(web.project.id, project.id);
    assert!(web.domains[0].tls);
    assert_eq!(web.volumes.len(), 1);
    assert!(web.container.is_none());

    let dup = CreateService {
        environment_id: staging.id.clone(),
        server_id: None,
        name: "web".into(),
        source: Source::Image {
            image: "nginx".into(),
        },
        port: None,
        command: None,
        restart: RestartPolicy::default(),
        health_path: None,
        env: vec![],
        domains: vec!["shop.example.com".into()],
        volumes: vec![],
        deploy: false,
    };
    let err = api.create_service(&dup).await.unwrap_err();
    assert!(err.to_string().contains("already in use"), "{err}");

    let err = api
        .set_env(
            &web.service.id,
            &[EnvVar {
                key: "1X".into(),
                value: String::new(),
                secret: false,
            }],
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("invalid environment variable"),
        "{err}"
    );
    let vars = vec![
        EnvVar {
            key: "B".into(),
            value: "2".into(),
            secret: true,
        },
        EnvVar {
            key: "A".into(),
            value: "1".into(),
            secret: false,
        },
    ];
    assert_eq!(api.set_env(&web.service.id, &vars).await.unwrap(), vars);

    let domain = api
        .add_domain(
            &web.service.id,
            &CreateDomain {
                host: "admin.example.com".into(),
                port: Some(8080),
                tls: false,
            },
        )
        .await
        .unwrap();
    assert!(!domain.tls);
    api.delete_domain(&domain.id).await.unwrap();
    assert!(api.delete_domain(&domain.id).await.is_err());

    let volume = api
        .add_volume(
            &web.service.id,
            &CreateVolume {
                name: "uploads".into(),
                mount_path: "/uploads".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        api.add_volume(
            &web.service.id,
            &CreateVolume {
                name: "uploads".into(),
                mount_path: "/x".into()
            }
        )
        .await
        .is_err()
    );
    assert!(
        api.add_volume(
            &web.service.id,
            &CreateVolume {
                name: "other".into(),
                mount_path: "relative".into()
            }
        )
        .await
        .is_err()
    );
    api.delete_volume(&volume.id, false).await.unwrap();

    let updated = api
        .update_service(
            &web.service.id,
            &UpdateService {
                port: Some(0),
                health_path: Some(String::new()),
                name: Some("frontend".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.service.port, None);
    assert_eq!(updated.service.health_path, None);
    assert_eq!(updated.service.name, "frontend");

    let db = api
        .create_service(&CreateService {
            environment_id: production.id.clone(),
            server_id: None,
            name: "db".into(),
            source: Source::Database {
                kind: DatabaseKind::Postgres,
                version: String::new(),
            },
            port: None,
            command: None,
            restart: RestartPolicy::default(),
            health_path: None,
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: false,
        })
        .await
        .unwrap();
    assert_eq!(db.service.port, Some(5432));
    assert_eq!(
        db.service.source,
        Source::Database {
            kind: DatabaseKind::Postgres,
            version: "18-alpine".into()
        }
    );
    assert_eq!(db.volumes[0].mount_path, "/var/lib/postgresql");
    let info = db.database.clone().expect("database info");
    assert_eq!((info.host.as_str(), info.port), ("db", 5432));
    assert_eq!(info.database.as_deref(), Some("db"));
    assert_eq!(
        info.internal_url,
        format!("postgres://db:{}@db:5432/db", info.password)
    );
    assert_eq!(info.public_url, None);
    assert!(api.backups(&db.service.id).await.unwrap().is_empty());
    let err = api.create_backup(&db.service.id).await.unwrap_err();
    assert!(err.to_string().contains("not been deployed"), "{err}");
    assert_eq!(
        api.backup_schedule(&db.service.id).await.unwrap(),
        BackupSchedule::default()
    );
    let schedule = BackupSchedule {
        enabled: true,
        hour_utc: 4,
        keep: 3,
    };
    assert_eq!(
        api.set_backup_schedule(&db.service.id, &schedule)
            .await
            .unwrap(),
        schedule
    );
    let bad = BackupSchedule {
        hour_utc: 24,
        ..schedule
    };
    assert!(api.set_backup_schedule(&db.service.id, &bad).await.is_err());
    let exposed = api
        .update_database(
            &db.service.id,
            &UpdateDatabase {
                public_port: Some(15432),
            },
        )
        .await
        .unwrap();
    assert_eq!(exposed.public_port, Some(15432));
    assert!(
        api.update_database(
            &db.service.id,
            &UpdateDatabase {
                public_port: Some(443)
            }
        )
        .await
        .is_err()
    );
    assert!(
        api.update_database(&updated.service.id, &UpdateDatabase::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("not a database service")
    );
    let password = db
        .env
        .iter()
        .find(|v| v.key == "POSTGRES_PASSWORD")
        .unwrap();
    assert!(password.secret && password.value.len() >= 16);

    assert!(
        api.deployments(&db.service.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let err = api.stop(&db.service.id).await.unwrap_err();
    assert!(err.to_string().contains("not been deployed"), "{err}");

    let activity = api.activity(100).await.unwrap();
    assert!(
        activity
            .iter()
            .any(|a| a.message == "Service created" && a.service_name.as_deref() == Some("db"))
    );
    assert!(
        activity.windows(2).all(|w| w[0].id > w[1].id),
        "activity is newest first"
    );

    api.delete_environment(&staging.id).await.unwrap();
    api.delete_project(&project.id).await.unwrap();
    assert!(api.projects().await.unwrap().is_empty());
    assert!(api.service(&db.service.id).await.is_err());
}

#[tokio::test]
async fn ssh_servers_can_be_registered_and_local_is_protected() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let api = daemon.client();
    let server = api
        .create_server(&CreateServer {
            name: "edge".into(),
            host: "203.0.113.7".into(),
            user: "deploy".into(),
            port: 2222,
            ssh_key_secret_id: None,
        })
        .await
        .unwrap();
    assert_eq!(server.kind, ServerKind::Ssh);
    assert_eq!(server.port, Some(2222));
    assert!(
        api.create_server(&CreateServer {
            name: "edge".into(),
            host: "x".into(),
            user: "root".into(),
            port: 22,
            ssh_key_secret_id: None,
        })
        .await
        .is_err()
    );
    let local = api
        .servers()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServerKind::Local)
        .unwrap();
    let err = api.delete_server(&local.id).await.unwrap_err();
    assert!(err.to_string().contains("cannot be removed"), "{err}");
    api.delete_server(&server.id).await.unwrap();
    let info = api.info().await.unwrap();
    assert!(info.ssh_public_key.unwrap().starts_with("ssh-ed25519 "));
}

#[tokio::test]
async fn token_rotation_applies_without_restart_and_init_is_guarded() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let old = daemon.client();
    old.projects().await.unwrap();

    let out = daemon.run(&["init"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already initialized"));

    let out = daemon.run(&["token", "rotate"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let token = stdout.lines().nth(1).unwrap().trim().to_string();
    assert_eq!(token.len(), 64);

    let err = old.projects().await.unwrap_err();
    assert!(err.to_string().contains("unauthorized"), "{err}");
    let new = client::Client::new(&daemon.url, &token).unwrap();
    new.projects().await.unwrap();
}

#[test]
fn serve_refuses_an_uninitialized_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(support::daemon_bin())
        .arg("--data-dir")
        .arg(dir.path())
        .arg("serve")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not initialized"));
}

#[tokio::test]
async fn settings_persist_and_the_api_rotates_the_token() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let c = daemon.client();

    let s = c
        .update_settings(&UpdateSettings {
            public_ip_override: Some("203.0.113.7".into()),
            manage_proxy: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(s.public_ip.as_deref(), Some("203.0.113.7"));
    assert!(!s.manage_proxy);
    assert_eq!(
        c.info().await.unwrap().public_ip.as_deref(),
        Some("203.0.113.7")
    );
    let config = std::fs::read_to_string(daemon.dir.path().join("config.toml")).unwrap();
    assert!(config.contains("public_ip_override = \"203.0.113.7\""));
    assert!(config.contains("manage_proxy = false"));

    let err = c
        .update_settings(&UpdateSettings {
            public_ip_override: Some("not an ip".into()),
            manage_proxy: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not an IP address"), "{err}");

    let s = c
        .update_settings(&UpdateSettings {
            public_ip_override: Some("auto".into()),
            manage_proxy: None,
        })
        .await
        .unwrap();
    assert_eq!(s.public_ip_override, None);
    assert!(!s.manage_proxy, "fields left out keep their value");

    let rotated = c.rotate_token().await.unwrap();
    assert!(
        c.info().await.is_err(),
        "the old token stops working at once"
    );
    let c = c.with_token(&rotated.token);
    assert_eq!(c.settings().await.unwrap().public_ip_override, None);
}

/// An install from before the Postgres store: `serve` finds the SQLite file, imports it into the
/// empty store, keeps the file as a backup and records the move in the activity feed.
#[tokio::test]
async fn serve_imports_a_sqlite_store_from_v0_1_0() {
    let Some(empty_store) = support::pg::database_url() else {
        return;
    };
    let mut sqlite = None;
    let Some(daemon) = Daemon::start_with(|dir| {
        let path = dir.join(format!("{}.db", common::brand::DIR));
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(include_str!("fixtures/v0.1.0-store.sql"))
            .unwrap();
        sqlite = Some(path);
        vec![(common::brand::env_var("DATABASE_URL"), empty_store)]
    })
    .await
    else {
        return;
    };
    let api = daemon.client();

    let projects = api.projects().await.unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "shop");
    let services: Vec<Service> = api
        .tree()
        .await
        .unwrap()
        .projects
        .into_iter()
        .flat_map(|p| p.environments)
        .flat_map(|e| e.services)
        .collect();
    let mut names: Vec<_> = services.iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["api", "web"]);
    let web = services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.primary_domain.as_deref(), Some("web.example.test"));
    let deployments = api.deployments(&web.id, 20).await.unwrap();
    assert_eq!(
        deployments
            .iter()
            .map(|d| (d.number, d.status))
            .collect::<Vec<_>>(),
        [
            (2, DeploymentStatus::Live),
            (1, DeploymentStatus::Superseded)
        ]
    );
    let detail = api.service(&web.id).await.unwrap();
    assert_eq!(
        detail
            .env
            .iter()
            .map(|v| (v.key.as_str(), v.value.as_str()))
            .collect::<Vec<_>>(),
        [("GREETING", "hello"), ("EXTRA", "1")]
    );
    assert_eq!(detail.volumes.len(), 1);

    let activity = api.activity(50).await.unwrap();
    assert_eq!(activity.len(), 10);
    assert_eq!(
        activity[0].message,
        "Moved the store from SQLite to Postgres"
    );
    assert_eq!(activity[0].id, 10);

    let sqlite = sqlite.unwrap();
    assert!(!sqlite.exists());
    let kept: Vec<_> = std::fs::read_dir(daemon.dir.path())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.contains(".db.migrated-") && name.ends_with(".bak"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
}

#[tokio::test]
async fn secrets_are_encrypted_masked_and_protected_while_in_use() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let api = daemon.client();
    let key_path = daemon.dir.path().join("master.key");
    assert_eq!(std::fs::read(&key_path).unwrap().len(), 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let token = api
        .create_secret(&CreateSecret {
            name: "api-token".into(),
            value: SecretValue::Generic {
                value: "hunter2-very-secret".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(token.kind, SecretKind::Generic);
    assert_eq!(token.public_part, None);
    let s3 = api
        .create_secret(&CreateSecret {
            name: "r2".into(),
            value: SecretValue::S3Credentials {
                access_key_id: " AKIAEXAMPLE ".into(),
                secret_access_key: "s3-secret-value".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(s3.public_part.as_deref(), Some("AKIAEXAMPLE"));
    let key = api.generate_ssh_key("deploy-key").await.unwrap();
    let public = key.public_part.clone().unwrap();
    assert!(public.starts_with("ssh-ed25519 "), "{public}");

    let err = api
        .create_secret(&CreateSecret {
            name: "bad-key".into(),
            value: SecretValue::SshKey {
                private_key: "definitely-not-a-key".into(),
            },
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("OpenSSH private key"), "{err}");
    assert!(!err.contains("definitely-not-a-key"), "{err}");
    let err = api
        .create_secret(&CreateSecret {
            name: "api-token".into(),
            value: SecretValue::Generic { value: "x".into() },
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");

    let http = reqwest::Client::new();
    let listed = http
        .get(format!("{}{API_PREFIX}/secrets", daemon.url))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for plaintext in ["hunter2-very-secret", "s3-secret-value", "PRIVATE KEY"] {
        assert!(
            !listed.contains(plaintext),
            "list leaked {plaintext}: {listed}"
        );
    }
    let one = http
        .get(format!("{}{API_PREFIX}/secrets/{}", daemon.url, token.id))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!one.contains("hunter2"), "{one}");

    let revealed = api.reveal_secret(&token.id).await.unwrap();
    assert_eq!(
        revealed.value,
        SecretValue::Generic {
            value: "hunter2-very-secret".into()
        }
    );
    let SecretValue::SshKey { private_key } = api.reveal_secret(&key.id).await.unwrap().value
    else {
        panic!("not an ssh key");
    };
    assert!(private_key.contains("BEGIN OPENSSH PRIVATE KEY"));

    let renamed = api
        .update_secret(
            &token.id,
            &UpdateSecret {
                name: Some("api-key".into()),
                value: Some(SecretValue::Generic {
                    value: "replaced-value".into(),
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "api-key");
    assert!(renamed.updated_at > token.updated_at);
    assert_eq!(
        api.reveal_secret(&token.id).await.unwrap().value,
        SecretValue::Generic {
            value: "replaced-value".into()
        }
    );
    let err = api
        .update_secret(
            &token.id,
            &UpdateSecret {
                name: None,
                value: Some(SecretValue::S3Credentials {
                    access_key_id: "a".into(),
                    secret_access_key: "b".into(),
                }),
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("same kind"), "{err}");

    let server = api
        .create_server(&CreateServer {
            name: "edge".into(),
            host: "203.0.113.7".into(),
            user: "root".into(),
            port: 22,
            ssh_key_secret_id: Some(key.id.clone()),
        })
        .await
        .unwrap();
    assert_eq!(server.ssh_key_secret_id.as_deref(), Some(key.id.as_str()));
    let err = api
        .create_server(&CreateServer {
            name: "edge2".into(),
            host: "203.0.113.8".into(),
            user: "root".into(),
            port: 22,
            ssh_key_secret_id: Some(token.id.clone()),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expected ssh key"), "{err}");

    let used = api.secret(&key.id).await.unwrap();
    assert_eq!(used.used_by.len(), 1);
    assert_eq!(used.used_by[0].kind, UsageKind::Server);
    assert_eq!(used.used_by[0].name, "edge");
    let res = http
        .delete(format!("{}{API_PREFIX}/secrets/{}", daemon.url, key.id))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body: ApiError = res.json().await.unwrap();
    assert!(body.error.contains("server edge"), "{}", body.error);

    let server = api
        .update_server(
            &server.id,
            &UpdateServer {
                ssh_key_secret_id: Some(String::new()),
            },
        )
        .await
        .unwrap();
    assert_eq!(server.ssh_key_secret_id, None);
    assert!(api.secret(&key.id).await.unwrap().used_by.is_empty());
    api.delete_secret(&key.id).await.unwrap();
    assert!(api.secret(&key.id).await.is_err());
    assert_eq!(api.secrets().await.unwrap().len(), 2);

    let activity = api.activity(50).await.unwrap();
    assert!(
        activity
            .iter()
            .any(|a| a.message == "Secret api-token revealed")
    );
    let feed = serde_json::to_string(&activity).unwrap();
    for plaintext in [
        "hunter2",
        "replaced-value",
        "s3-secret-value",
        "PRIVATE KEY",
    ] {
        assert!(!feed.contains(plaintext), "activity leaked {plaintext}");
    }
}
