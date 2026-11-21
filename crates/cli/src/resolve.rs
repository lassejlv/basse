//! Turning names typed on the command line into tree nodes.

use anyhow::{Result, anyhow, bail};
use common::*;

pub type Found<'a> = (&'a ProjectNode, &'a EnvironmentNode, &'a Service);

/// `web`, `production/web`, `acme/production/web` or a service id.
pub fn service<'a>(tree: &'a Tree, query: &str) -> Result<Found<'a>> {
    let parts: Vec<&str> = query.split('/').collect();
    let all = tree.projects.iter().flat_map(|p| {
        p.environments
            .iter()
            .flat_map(move |e| e.services.iter().map(move |s| (p, e, s)))
    });
    let matches: Vec<Found> = all
        .filter(|(p, e, s)| match parts.as_slice() {
            [name] => s.name == *name || s.id == *name,
            [env, name] => e.environment.name == *env && s.name == *name,
            [project, env, name] => {
                p.project.name == *project && e.environment.name == *env && s.name == *name
            }
            _ => false,
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no service '{query}'"),
        many => {
            let options: Vec<String> = many
                .iter()
                .map(|(p, e, s)| format!("{}/{}/{}", p.project.name, e.environment.name, s.name))
                .collect();
            bail!("'{query}' is ambiguous, use one of: {}", options.join(", "))
        }
    }
}

pub fn project<'a>(tree: &'a Tree, name: &str) -> Result<&'a ProjectNode> {
    tree.projects
        .iter()
        .find(|p| p.project.name == name || p.project.id == name)
        .ok_or_else(|| anyhow!("no project '{name}'"))
}

/// `acme/staging`, or just `acme` for its production (or only) environment.
pub fn environment<'a>(
    tree: &'a Tree,
    query: &str,
) -> Result<(&'a ProjectNode, &'a EnvironmentNode)> {
    let (project_name, env_name) = match query.split_once('/') {
        Some((p, e)) => (p, Some(e)),
        None => (query, None),
    };
    let p = project(tree, project_name)?;
    let env = match env_name {
        Some(name) => p.environments.iter().find(|e| e.environment.name == name),
        None => p
            .environments
            .iter()
            .find(|e| e.environment.name == "production")
            .or_else(|| (p.environments.len() == 1).then(|| &p.environments[0])),
    };
    env.map(|e| (p, e)).ok_or_else(|| match env_name {
        Some(name) => anyhow!("project '{project_name}' has no environment '{name}'"),
        None => anyhow!(
            "project '{project_name}' has several environments: use {project_name}/<environment>"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn tree() -> Tree {
        let now = Utc::now();
        let svc = |id: &str, name: &str, env: &str| Service {
            id: id.into(),
            environment_id: env.into(),
            server_id: "local".into(),
            name: name.into(),
            source: Source::Image {
                image: "nginx".into(),
            },
            port: None,
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            created_at: now,
            state: ServiceState::Running,
            primary_domain: None,
            last_deployed_at: None,
            live_deployment_id: None,
            replicas: 1,
            scaling: Default::default(),
        };
        let env = |id: &str, name: &str, services: Vec<Service>| EnvironmentNode {
            environment: Environment {
                id: id.into(),
                project_id: "p".into(),
                name: name.into(),
                created_at: now,
            },
            services,
        };
        Tree {
            projects: vec![ProjectNode {
                project: Project {
                    id: "p".into(),
                    name: "acme".into(),
                    description: None,
                    created_at: now,
                },
                environments: vec![
                    env(
                        "e1",
                        "production",
                        vec![svc("s1", "web", "e1"), svc("s2", "api", "e1")],
                    ),
                    env("e2", "staging", vec![svc("s3", "web", "e2")]),
                ],
            }],
        }
    }

    #[test]
    fn unique_names_resolve_directly() {
        let t = tree();
        assert_eq!(service(&t, "api").unwrap().2.id, "s2");
        assert_eq!(service(&t, "s3").unwrap().2.id, "s3");
    }

    #[test]
    fn ambiguous_names_need_a_path() {
        let t = tree();
        let err = service(&t, "web").unwrap_err().to_string();
        assert!(err.contains("acme/production/web") && err.contains("acme/staging/web"));
        assert_eq!(service(&t, "staging/web").unwrap().2.id, "s3");
        assert_eq!(service(&t, "acme/production/web").unwrap().2.id, "s1");
    }

    #[test]
    fn environments_default_to_production() {
        let t = tree();
        assert_eq!(environment(&t, "acme").unwrap().1.environment.id, "e1");
        assert_eq!(
            environment(&t, "acme/staging").unwrap().1.environment.id,
            "e2"
        );
        assert!(environment(&t, "acme/dev").is_err());
    }
}
