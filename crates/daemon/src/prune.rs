//! Reclaiming disk space: old build images, dangling images and the build cache.

use crate::docker;
use anyhow::{Context as _, Result};
use bollard::Docker;
use bollard::query_parameters::{
    DataUsageOptions, ListImagesOptions, PruneBuildOptions, PruneImagesOptions, RemoveImageOptions,
};
use common::{Deployment, DeploymentStatus, Id, PruneResult, Reclaimable, brand};
use std::collections::HashMap;

/// Splits a build tag `<prefix>/<service id>:<number>` into the service id and number.
pub fn parse_build_tag(tag: &str) -> Option<(&str, i64)> {
    let rest = tag.strip_prefix(brand::PREFIX)?.strip_prefix('/')?;
    let (service, number) = rest.rsplit_once(':')?;
    Some((service, number.parse().ok()?))
}

/// Deployments whose images a service keeps: the live one, the one a rollback returns to, the
/// newest, and any still in progress.
fn kept(deployments: &[Deployment]) -> Vec<&Deployment> {
    let live = deployments
        .iter()
        .filter(|d| d.status == DeploymentStatus::Live)
        .map(|d| d.number)
        .max();
    let previous = deployments
        .iter()
        .filter(|d| d.status == DeploymentStatus::Superseded && live.is_none_or(|l| d.number < l))
        .map(|d| d.number)
        .max();
    let newest = deployments.iter().map(|d| d.number).max();
    deployments
        .iter()
        .filter(|d| !d.status.is_finished() || [live, previous, newest].contains(&Some(d.number)))
        .collect()
}

/// Build tags safe to remove: every tag of a deleted service, and the tags of a service that are
/// older than the deployments it keeps and not run by any of them (a rollback runs an older
/// tag). Other images are never touched.
pub fn select(tags: &[String], deployments: &HashMap<Id, Vec<Deployment>>) -> Vec<String> {
    tags.iter()
        .filter(|tag| {
            let Some((service, number)) = parse_build_tag(tag) else {
                return false;
            };
            let Some(list) = deployments.get(service) else {
                return true;
            };
            let kept = kept(list);
            let older = kept
                .iter()
                .map(|d| d.number)
                .min()
                .is_some_and(|min| number < min);
            older
                && !kept
                    .iter()
                    .any(|d| d.image.as_deref() == Some(tag.as_str()))
        })
        .cloned()
        .collect()
}

/// Bytes an image frees when removed: its size minus layers shared with other images.
fn own_size(size: i64, shared: i64) -> u64 {
    u64::try_from(size - shared.max(0)).unwrap_or(0)
}

struct Candidates {
    /// Build tags to remove with the bytes each frees.
    tags: Vec<(String, u64)>,
    dangling: Vec<u64>,
}

async fn candidates(
    docker: &Docker,
    deployments: &HashMap<Id, Vec<Deployment>>,
) -> Result<Candidates> {
    let images = docker
        .list_images(Some(ListImagesOptions {
            shared_size: true,
            ..Default::default()
        }))
        .await
        .context("cannot list images")?;
    let build_tags: Vec<String> = images
        .iter()
        .flat_map(|i| &i.repo_tags)
        .filter(|t| parse_build_tag(t).is_some())
        .cloned()
        .collect();
    let selected = select(&build_tags, deployments);
    let tags = images
        .iter()
        .flat_map(|i| {
            let size = own_size(i.size, i.shared_size);
            let frees = i.containers <= 0 && i.repo_tags.iter().all(|t| selected.contains(t));
            i.repo_tags
                .iter()
                .filter(|t| selected.contains(t))
                .map(move |t| (t.clone(), if frees { size } else { 0 }))
                .collect::<Vec<_>>()
        })
        .collect();
    let dangling = images
        .iter()
        .filter(|i| i.containers <= 0 && i.repo_tags.iter().all(|t| t == "<none>:<none>"))
        .map(|i| own_size(i.size, i.shared_size))
        .collect();
    Ok(Candidates { tags, dangling })
}

async fn build_cache_bytes(docker: &Docker) -> u64 {
    let options = DataUsageOptions {
        _type: Some(vec!["build-cache".into()]),
        ..Default::default()
    };
    docker
        .df(Some(options))
        .await
        .ok()
        .and_then(|df| df.build_cache_usage)
        .and_then(|u| u.reclaimable)
        .and_then(|b| u64::try_from(b).ok())
        .unwrap_or(0)
}

/// What `prune` would free on this server.
pub async fn reclaimable(
    docker: &Docker,
    deployments: &HashMap<Id, Vec<Deployment>>,
) -> Result<Reclaimable> {
    let c = candidates(docker, deployments).await?;
    Ok(Reclaimable {
        images: c.tags.len() + c.dangling.len(),
        image_bytes: c.tags.iter().map(|(_, b)| b).sum::<u64>() + c.dangling.iter().sum::<u64>(),
        build_cache_bytes: build_cache_bytes(docker).await,
    })
}

/// Removes old build images, then dangling images and the whole unused build cache.
pub async fn prune(
    docker: &Docker,
    deployments: &HashMap<Id, Vec<Deployment>>,
) -> Result<PruneResult> {
    let c = candidates(docker, deployments).await?;
    let mut result = PruneResult {
        reclaimed_bytes: 0,
        removed_images: Vec::new(),
    };
    for (tag, bytes) in c.tags {
        match docker
            .remove_image(&tag, None::<RemoveImageOptions>, None)
            .await
        {
            Ok(_) => {
                result.reclaimed_bytes += bytes;
                result.removed_images.push(tag);
            }
            Err(e) if docker::status_code(&e) == Some(409) => {
                tracing::info!("keeping {tag}: {e}");
            }
            Err(e) if docker::is_not_found(&e) => {}
            Err(e) => return Err(e).with_context(|| format!("cannot remove {tag}")),
        }
    }
    let dangling = docker
        .prune_images(Some(PruneImagesOptions {
            filters: Some(HashMap::from([(
                "dangling".to_string(),
                vec!["true".to_string()],
            )])),
        }))
        .await
        .context("cannot prune dangling images")?;
    result.reclaimed_bytes += dangling
        .space_reclaimed
        .and_then(|b| u64::try_from(b).ok())
        .unwrap_or(0);
    result.removed_images.extend(
        dangling
            .images_deleted
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| i.deleted)
            .map(|id| docker::short_id(id.trim_start_matches("sha256:")).to_string()),
    );
    let cache = docker
        .prune_build(Some(PruneBuildOptions {
            all: Some(true),
            ..Default::default()
        }))
        .await
        .context("cannot prune the build cache")?;
    result.reclaimed_bytes += cache
        .space_reclaimed
        .and_then(|b| u64::try_from(b).ok())
        .unwrap_or(0);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn d(number: i64, status: DeploymentStatus) -> Deployment {
        Deployment {
            id: format!("d{number}"),
            service_id: "svc".into(),
            number,
            status,
            trigger: "cli".into(),
            commit: None,
            commit_message: None,
            image: Some(docker::build_tag("svc", number)),
            container_id: None,
            steps: Vec::new(),
            error: None,
            created_at: Utc::now(),
            finished_at: None,
        }
    }

    fn tags(service: &str, numbers: &[i64]) -> Vec<String> {
        numbers
            .iter()
            .map(|n| docker::build_tag(service, *n))
            .collect()
    }

    #[test]
    fn build_tags_parse() {
        assert_eq!(
            parse_build_tag(&docker::build_tag("abc", 12)),
            Some(("abc", 12))
        );
        assert_eq!(parse_build_tag("nginx:latest"), None);
        assert_eq!(
            parse_build_tag(&format!("{}/abc:latest", brand::PREFIX)),
            None
        );
    }

    #[test]
    fn prune_keeps_live_previous_and_newer_images() {
        use DeploymentStatus::*;
        let deployments = HashMap::from([(
            "svc".to_string(),
            vec![
                d(1, Superseded),
                d(2, Superseded),
                d(3, Failed),
                d(4, Superseded),
                d(5, Live),
                d(6, Failed),
            ],
        )]);
        let selected = select(&tags("svc", &[1, 2, 3, 4, 5, 6]), &deployments);
        assert_eq!(selected, tags("svc", &[1, 2, 3]));
    }

    #[test]
    fn prune_keeps_the_older_image_a_rollback_runs() {
        use DeploymentStatus::*;
        let mut rollback = d(5, Live);
        rollback.image = Some(docker::build_tag("svc", 3));
        let deployments = HashMap::from([(
            "svc".to_string(),
            vec![
                d(1, Superseded),
                d(2, Superseded),
                d(3, Superseded),
                d(4, Superseded),
                rollback,
            ],
        )]);
        assert_eq!(
            select(&tags("svc", &[1, 2, 3, 4]), &deployments),
            tags("svc", &[1, 2])
        );
    }

    #[test]
    fn prune_never_touches_running_deployments_or_other_images() {
        use DeploymentStatus::*;
        let deployments = HashMap::from([(
            "svc".to_string(),
            vec![d(1, Superseded), d(2, Live), d(3, Building)],
        )]);
        let mut all = tags("svc", &[1, 2, 3]);
        all.push("postgres:17-alpine".into());
        assert!(select(&all, &deployments).is_empty());
    }

    #[test]
    fn prune_removes_images_of_deleted_services() {
        let deployments = HashMap::from([("svc".to_string(), Vec::new())]);
        let all = [tags("svc", &[1]), tags("gone", &[1, 2])].concat();
        assert_eq!(select(&all, &deployments), tags("gone", &[1, 2]));
    }

    #[test]
    fn prune_without_a_live_deployment_keeps_the_last_two() {
        use DeploymentStatus::*;
        let deployments = HashMap::from([(
            "svc".to_string(),
            vec![d(1, Superseded), d(2, Superseded), d(3, Failed)],
        )]);
        assert_eq!(
            select(&tags("svc", &[1, 2, 3]), &deployments),
            tags("svc", &[1])
        );
    }
}
