//! Database backups to S3 against real Docker and MinIO containers. These run in CI (they need
//! no free ports), unlike the ignored end-to-end tests.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::process::Command;
use std::time::{Duration, Instant};
use support::{Daemon, minio};

async fn wait_backup(api: &Client, id: &str) -> Backup {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let b = api.backup(id).await.unwrap();
        if b.status != BackupStatus::Running {
            return b;
        }
        assert!(Instant::now() < deadline, "backup never finished: {b:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Waits until the service's newest deployment is live and its container runs.
async fn wait_running(api: &Client, service_id: &str) -> ServiceDetail {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let detail = api.service(service_id).await.unwrap();
        if detail.service.state == ServiceState::Running && detail.container.is_some() {
            return detail;
        }
        assert!(
            !matches!(detail.service.state, ServiceState::Failed),
            "deploy failed: {:?}",
            api.deployments(service_id, 1).await.unwrap()
        );
        assert!(Instant::now() < deadline, "service never ran: {detail:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Runs SQL in the database container, retrying while Postgres is still starting.
async fn psql(container: &str, info: &DatabaseInfo, sql: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let out = Command::new("docker")
            .args(["exec", container, "psql", "-v", "ON_ERROR_STOP=1", "-tA"])
            .args(["-U", info.username.as_deref().unwrap()])
            .args(["-d", info.database.as_deref().unwrap(), "-c", sql])
            .output()
            .unwrap();
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
        assert!(
            Instant::now() < deadline,
            "{sql}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn objects(endpoint: &str, bucket: &str, prefix: &str) -> Vec<(String, u64)> {
    minio::client(endpoint, bucket)
        .list(prefix.into(), None)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|p| p.contents)
        .map(|o| (o.key, o.size))
        .collect()
}

#[tokio::test]
async fn postgres_backups_go_to_s3_and_restore_from_it() {
    let Some(daemon) = Daemon::start_on_host().await else {
        return;
    };
    let Some((endpoint, bucket)) = minio::bucket().await else {
        return;
    };
    let api = daemon.client();

    let creds = api
        .create_secret(&CreateSecret {
            name: "minio".into(),
            value: SecretValue::S3Credentials {
                access_key_id: minio::ACCESS_KEY.into(),
                secret_access_key: minio::SECRET_KEY.into(),
            },
        })
        .await
        .unwrap();
    let storage = api
        .create_storage(&CreateStorage {
            name: "offsite".into(),
            endpoint: endpoint.clone(),
            region: minio::REGION.into(),
            bucket: bucket.clone(),
            path_prefix: "/db/".into(),
            path_style: true,
            credentials_secret_id: creds.id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(storage.path_prefix, "db");
    let test = api.test_storage(&storage.id).await.unwrap();
    assert!(test.ok, "{test:?}");
    assert!(test.latency_ms.is_some());
    assert!(
        objects(&endpoint, &bucket, "").await.is_empty(),
        "the probe is cleaned up"
    );
    assert_eq!(
        api.storage(&storage.id).await.unwrap().last_test,
        Some(test)
    );
    let used = api.secret(&creds.id).await.unwrap().used_by;
    assert_eq!(used[0].kind, UsageKind::Storage);
    assert_eq!(used[0].name, "offsite");

    let project = api
        .create_project(&CreateProject {
            name: "s3".into(),
            description: None,
        })
        .await
        .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let created = api
        .create_service(&CreateService {
            environment_id: env.id,
            server_id: None,
            name: "pg".into(),
            source: Source::Database {
                kind: DatabaseKind::Postgres,
                version: String::new(),
            },
            port: None,
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            env: vec![],
            domains: vec![],
            volumes: vec![],
            deploy: true,
            replicas: None,
        })
        .await
        .unwrap();
    let sid = created.service.id.clone();
    let detail = wait_running(&api, &sid).await;
    let info = detail.database.clone().unwrap();
    let container = detail.container.unwrap().id;
    psql(
        &container,
        &info,
        "CREATE TABLE notes (t text); INSERT INTO notes VALUES ('kept')",
    )
    .await;

    let started = api.create_backup(&sid, Some(&storage.id)).await.unwrap();
    assert_eq!(started.storage, BackupStorage::S3);
    assert_eq!(started.storage_id.as_deref(), Some(storage.id.as_str()));
    let backup = wait_backup(&api, &started.id).await;
    assert_eq!(backup.status, BackupStatus::Done, "{backup:?}");
    let key = format!("db/{sid}/{}", backup.file_name);
    assert_eq!(
        objects(&endpoint, &bucket, "db/").await,
        vec![(key.clone(), backup.size_bytes)]
    );
    assert!(
        !daemon.dir.path().join("backups").join(&sid).exists(),
        "nothing is kept locally"
    );

    let mut download = api.download_backup(&backup.id).await.unwrap();
    let mut bytes = Vec::new();
    while let Some(chunk) = download.next().await {
        bytes.extend(chunk.unwrap());
    }
    assert_eq!(bytes.len() as u64, backup.size_bytes);
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "a gzip file");

    psql(&container, &info, "UPDATE notes SET t = 'changed'").await;
    api.restore_backup(&backup.id).await.unwrap();
    assert_eq!(psql(&container, &info, "SELECT t FROM notes").await, "kept");

    let res = reqwest::Client::new()
        .delete(format!("{}{API_PREFIX}/storage/{}", daemon.url, storage.id))
        .bearer_auth(&daemon.token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::CONFLICT);

    let schedule = api
        .set_backup_schedule(
            &sid,
            &BackupSchedule {
                enabled: true,
                hour_utc: 3,
                keep: 2,
                storage_id: Some(storage.id.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(schedule.storage_id.as_deref(), Some(storage.id.as_str()));
    assert_eq!(api.backup_schedule(&sid).await.unwrap(), schedule);

    api.delete_backup(&backup.id).await.unwrap();
    assert!(objects(&endpoint, &bucket, "db/").await.is_empty());
    api.delete_service(&sid, true).await.unwrap();
    api.delete_storage(&storage.id).await.unwrap();
    api.delete_project(&project.id).await.unwrap();
}
