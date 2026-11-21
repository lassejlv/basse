//! Instance backups to MinIO and `restore` into a fresh data directory and database. These run
//! in CI.

mod support;

use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt as _;
use std::process::Command;
use std::time::{Duration, Instant};
use support::{Daemon, daemon_cmd, minio, pg};

const PASSPHRASE: &str = "correct horse battery staple";

async fn wait_instance_backup(api: &Client, id: &str) -> InstanceBackup {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let b = api
            .instance_backups()
            .await
            .unwrap()
            .into_iter()
            .find(|b| b.id == id)
            .unwrap();
        if b.status != BackupStatus::Running {
            return b;
        }
        assert!(
            Instant::now() < deadline,
            "instance backup never finished: {b:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

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

async fn wait_running(api: &Client, service_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let detail = api.service(service_id).await.unwrap();
        if detail.service.state == ServiceState::Running && detail.container.is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "service never ran: {detail:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn instance_backup_restores_into_a_fresh_data_dir() {
    let Some(old) = Daemon::start().await else {
        return;
    };
    let Some((endpoint, bucket)) = minio::bucket().await else {
        return;
    };
    let api = old.client();
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
    let token = api
        .create_secret(&CreateSecret {
            name: "api-token".into(),
            value: SecretValue::Generic {
                value: "survives-the-move".into(),
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
            path_prefix: String::new(),
            path_style: true,
            credentials_secret_id: creds.id.clone(),
        })
        .await
        .unwrap();
    let project = api
        .create_project(&CreateProject {
            name: "moving".into(),
            description: Some("restored on a new machine".into()),
        })
        .await
        .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    let db = api
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
        })
        .await
        .unwrap();
    let sid = db.service.id.clone();
    wait_running(&api, &sid).await;
    let db_backup = api.create_backup(&sid, Some(&storage.id)).await.unwrap();
    let db_backup = wait_backup(&api, &db_backup.id).await;
    assert_eq!(db_backup.status, BackupStatus::Done, "{db_backup:?}");

    let schedule = api
        .set_instance_backup_schedule(&UpdateInstanceBackupSchedule {
            enabled: true,
            hour_utc: 2,
            keep: 3,
            storage_id: Some(storage.id.clone()),
            passphrase: Some(PASSPHRASE.into()),
        })
        .await
        .unwrap();
    assert!(schedule.has_passphrase);
    let raw = serde_json::to_string(&api.instance_backup_schedule().await.unwrap()).unwrap();
    assert!(!raw.contains(PASSPHRASE), "{raw}");
    let err = api
        .create_instance_backup(&CreateInstanceBackup {
            storage_id: Some(storage.id.clone()),
            passphrase: "short".into(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("at least 12"), "{err}");
    let started = api
        .create_instance_backup(&CreateInstanceBackup {
            storage_id: Some(storage.id.clone()),
            passphrase: PASSPHRASE.into(),
        })
        .await
        .unwrap();
    let backup = wait_instance_backup(&api, &started.id).await;
    assert_eq!(backup.status, BackupStatus::Done, "{backup:?}");
    assert_eq!(backup.storage, BackupStorage::S3);
    let key = format!("instance/{}", backup.file_name);
    let object = minio::client(&endpoint, &bucket)
        .get_object(&key)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(object.len() as u64, backup.size_bytes);
    let master_key = std::fs::read(old.dir.path().join("master.key")).unwrap();
    assert!(
        !object.windows(master_key.len()).any(|w| w == master_key),
        "the archive is encrypted"
    );
    let mut download = api.download_instance_backup(&backup.id).await.unwrap();
    let mut downloaded = Vec::new();
    while let Some(chunk) = download.next().await {
        downloaded.extend(chunk.unwrap());
    }
    assert!(downloaded == object, "download serves the stored archive");
    let old_token = old.token.clone();
    drop(old);

    let new_dir = tempfile::tempdir().unwrap();
    let new_store = pg::database_url().unwrap();
    let passphrase_file = new_dir.path().join("passphrase");
    std::fs::write(&passphrase_file, format!("{PASSPHRASE}\n")).unwrap();
    let restore = |passphrase_file: &std::path::Path| {
        let mut cmd: Command = daemon_cmd(&new_dir.path().join("data"));
        cmd.args(["restore", &format!("s3://{bucket}/{key}")])
            .arg("--passphrase-file")
            .arg(passphrase_file)
            .args(["--s3-endpoint", &endpoint, "--s3-region", minio::REGION])
            .args([
                "--s3-path-style",
                "--no-service",
                "--database-url",
                &new_store,
            ])
            .env("AWS_ACCESS_KEY_ID", minio::ACCESS_KEY)
            .env("AWS_SECRET_ACCESS_KEY", minio::SECRET_KEY);
        cmd.output().unwrap()
    };
    let wrong = new_dir.path().join("wrong");
    std::fs::write(&wrong, "not the passphrase at all").unwrap();
    let out = restore(&wrong);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("wrong passphrase"), "{stderr}");

    let out = restore(&passphrase_file);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "restore failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("2 secrets decrypt with the restored master key"),
        "{stdout}"
    );
    assert!(
        stdout.contains("local "),
        "the local server was checked: {stdout}"
    );
    let data = new_dir.path().join("data");
    assert_eq!(std::fs::read(data.join("master.key")).unwrap(), master_key);
    assert!(data.join("ssh/id_ed25519").exists());

    let restored_dir = tempfile::TempDir::new().unwrap();
    std::fs::remove_dir(restored_dir.path()).unwrap();
    std::fs::rename(&data, restored_dir.path()).unwrap();
    let new = Daemon::serve(restored_dir, old_token, Vec::new()).await;
    let api = new.client();
    let tree = api.tree().await.unwrap();
    assert_eq!(tree.projects.len(), 1);
    assert_eq!(tree.projects[0].project.id, project.id);
    assert_eq!(tree.projects[0].environments[0].services[0].id, sid);
    assert_eq!(
        api.reveal_secret(&token.id).await.unwrap().value,
        SecretValue::Generic {
            value: "survives-the-move".into()
        }
    );
    assert_eq!(api.storage(&storage.id).await.unwrap().name, "offsite");
    let backups = api.backups(&sid).await.unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, db_backup.id);
    let mut download = api.download_backup(&db_backup.id).await.unwrap();
    let mut bytes = 0;
    while let Some(chunk) = download.next().await {
        bytes += chunk.unwrap().len() as u64;
    }
    assert_eq!(
        bytes, db_backup.size_bytes,
        "the S3 backup is reachable after the move"
    );
    assert!(api.instance_backup_schedule().await.unwrap().has_passphrase);
    let restored_backup = &api.instance_backups().await.unwrap()[0];
    assert_eq!(restored_backup.id, backup.id);
    assert_eq!(
        restored_backup.status,
        BackupStatus::Done,
        "the backup the archive came from is not left looking interrupted"
    );
    assert_eq!(restored_backup.size_bytes, backup.size_bytes);
    assert!(
        api.activity(20)
            .await
            .unwrap()
            .iter()
            .any(|a| a.message == "Instance restored from a backup")
    );
    api.delete_service(&sid, true).await.unwrap();
}
