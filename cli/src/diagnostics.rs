use crate::compose::{check_docker_available, CommandResult, Deployment};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub install_dir: String,
    pub omni_version: Option<String>,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn summary_counts(&self) -> (usize, usize, usize, usize) {
        let mut ok = 0;
        let mut warnings = 0;
        let mut errors = 0;
        let mut info = 0;
        for check in &self.checks {
            match check.status {
                CheckStatus::Ok => ok += 1,
                CheckStatus::Warning => warnings += 1,
                CheckStatus::Error => errors += 1,
                CheckStatus::Info => info += 1,
            }
        }
        (ok, warnings, errors, info)
    }
}

pub async fn run_doctor(deployment: &Deployment, verbose: bool, logs_since: &str) -> DoctorReport {
    let mut checks = Vec::new();

    checks.extend(check_files(deployment));
    checks.extend(check_tools());
    checks.extend(check_compose_ps(deployment));
    checks.extend(check_image_tags(deployment));
    checks.extend(check_internal_health(deployment, verbose));
    checks.extend(check_connector_manager(deployment, verbose));
    checks.extend(check_postgres_syncs(deployment));
    checks.extend(check_logs(deployment, logs_since));

    DoctorReport {
        install_dir: deployment.root.display().to_string(),
        omni_version: deployment.env.value("OMNI_VERSION"),
        checks,
    }
}

fn check_files(deployment: &Deployment) -> Vec<Check> {
    let mut checks = Vec::new();
    for (name, path, required) in [
        (".env", &deployment.env_file, true),
        ("docker/docker-compose.yml", &deployment.compose_file, true),
        (".env.example", &deployment.env_example, false),
        (
            "docker/docker-compose.local-inference.yml",
            &deployment.local_inference_compose_file,
            false,
        ),
        ("Caddyfile", &deployment.caddyfile, false),
    ] {
        let exists = path.exists();
        checks.push(Check {
            name: format!("file:{name}"),
            status: if exists {
                CheckStatus::Ok
            } else if required {
                CheckStatus::Error
            } else {
                CheckStatus::Warning
            },
            message: if exists {
                format!("{} exists", path.display())
            } else {
                format!("{} is missing", path.display())
            },
            details: None,
        });
    }
    checks
}

fn check_tools() -> Vec<Check> {
    match check_docker_available() {
        Ok(results) => results
            .into_iter()
            .map(|result| command_check("tool", result))
            .collect(),
        Err(error) => vec![Check {
            name: "tool:docker".into(),
            status: CheckStatus::Error,
            message: error.to_string(),
            details: None,
        }],
    }
}

fn check_compose_ps(deployment: &Deployment) -> Vec<Check> {
    match deployment.compose_output(["ps", "--format", "json"]) {
        Ok(result) if result.success => parse_compose_ps(&result.stdout),
        Ok(result) => vec![command_check("compose:ps", result)],
        Err(error) => vec![Check {
            name: "compose:ps".into(),
            status: CheckStatus::Error,
            message: error.to_string(),
            details: None,
        }],
    }
}

fn parse_compose_ps(stdout: &str) -> Vec<Check> {
    let mut values = Vec::new();
    if let Ok(Value::Array(array)) = serde_json::from_str::<Value>(stdout) {
        values = array;
    } else {
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                values.push(value);
            }
        }
    }

    if values.is_empty() {
        return vec![Check {
            name: "compose:services".into(),
            status: CheckStatus::Warning,
            message: "no Compose services reported; is the stack running?".into(),
            details: Some(stdout.to_string()),
        }];
    }

    values
        .into_iter()
        .map(|service| {
            let name =
                text_field(&service, &["Service", "Name"]).unwrap_or_else(|| "unknown".into());
            let state = text_field(&service, &["State"]).unwrap_or_else(|| "unknown".into());
            let health = text_field(&service, &["Health"]).unwrap_or_default();
            let good_state = state.eq_ignore_ascii_case("running")
                || state.eq_ignore_ascii_case("exited") && name == "migrator";
            let good_health = health.is_empty() || health.eq_ignore_ascii_case("healthy");
            Check {
                name: format!("service:{name}"),
                status: if good_state && good_health {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                message: if health.is_empty() {
                    format!("state={state}")
                } else {
                    format!("state={state}, health={health}")
                },
                details: None,
            }
        })
        .collect()
}

fn check_image_tags(deployment: &Deployment) -> Vec<Check> {
    let expected = deployment.env.value("OMNI_VERSION").unwrap_or_default();
    if expected.is_empty() {
        return vec![Check {
            name: "version:env".into(),
            status: CheckStatus::Warning,
            message: "OMNI_VERSION is not set in .env".into(),
            details: None,
        }];
    }

    match deployment.compose_output(["config", "--images"]) {
        Ok(result) if result.success => {
            let mismatches: Vec<_> = result
                .stdout
                .lines()
                .filter(|image| {
                    image.contains("ghcr.io/getomnico/omni/")
                        && !image.ends_with(&format!(":{expected}"))
                })
                .collect();
            if mismatches.is_empty() {
                vec![Check {
                    name: "version:images".into(),
                    status: CheckStatus::Ok,
                    message: format!("managed images resolve to OMNI_VERSION={expected}"),
                    details: None,
                }]
            } else {
                vec![Check {
                    name: "version:images".into(),
                    status: CheckStatus::Warning,
                    message: "some managed images do not match OMNI_VERSION".into(),
                    details: Some(mismatches.join("\n")),
                }]
            }
        }
        Ok(result) => vec![command_check("version:images", result)],
        Err(error) => vec![Check {
            name: "version:images".into(),
            status: CheckStatus::Warning,
            message: error.to_string(),
            details: None,
        }],
    }
}

fn check_internal_health(deployment: &Deployment, verbose: bool) -> Vec<Check> {
    let services = [
        ("searcher", "SEARCHER_PORT"),
        ("indexer", "INDEXER_PORT"),
        ("connector-manager", "CONNECTOR_MANAGER_PORT"),
        ("sandbox", "SANDBOX_PORT"),
    ];

    services
        .into_iter()
        .map(|(service, port_key)| {
            let port = deployment.env.value(port_key).unwrap_or_default();
            if port.is_empty() {
                return Check {
                    name: format!("health:{service}"),
                    status: CheckStatus::Warning,
                    message: format!("{port_key} is missing"),
                    details: None,
                };
            }
            let script = format!(
                "wget -qO- http://localhost:{port}/health 2>/dev/null || curl -fsS http://localhost:{port}/health"
            );
            match deployment.compose_output(["exec", "-T", service, "sh", "-c", &script]) {
                Ok(result) if result.success => Check {
                    name: format!("health:{service}"),
                    status: CheckStatus::Ok,
                    message: "health endpoint responded".into(),
                    details: verbose.then_some(result.stdout),
                },
                Ok(result) => Check {
                    name: format!("health:{service}"),
                    status: CheckStatus::Warning,
                    message: "health endpoint did not respond".into(),
                    details: Some(trim_details(result.stderr, result.stdout)),
                },
                Err(error) => Check {
                    name: format!("health:{service}"),
                    status: CheckStatus::Warning,
                    message: error.to_string(),
                    details: None,
                },
            }
        })
        .collect()
}

fn check_connector_manager(deployment: &Deployment, verbose: bool) -> Vec<Check> {
    let port = deployment
        .env
        .value("CONNECTOR_MANAGER_PORT")
        .unwrap_or_else(|| "3004".into());
    let mut checks = Vec::new();
    for endpoint in ["sources", "connectors"] {
        let script = format!(
            "wget -qO- http://localhost:{port}/{endpoint} 2>/dev/null || curl -fsS http://localhost:{port}/{endpoint}"
        );
        match deployment.compose_output(["exec", "-T", "connector-manager", "sh", "-c", &script]) {
            Ok(result) if result.success => checks.push(analyze_connector_manager_payload(
                endpoint,
                &result.stdout,
                verbose,
            )),
            Ok(result) => checks.push(Check {
                name: format!("connector-manager:{endpoint}"),
                status: CheckStatus::Warning,
                message: format!("could not fetch /{endpoint}"),
                details: Some(trim_details(result.stderr, result.stdout)),
            }),
            Err(error) => checks.push(Check {
                name: format!("connector-manager:{endpoint}"),
                status: CheckStatus::Warning,
                message: error.to_string(),
                details: None,
            }),
        }
    }
    checks
}

fn analyze_connector_manager_payload(endpoint: &str, stdout: &str, verbose: bool) -> Check {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout) else {
        return Check {
            name: format!("connector-manager:{endpoint}"),
            status: CheckStatus::Info,
            message: format!("/{endpoint} responded"),
            details: verbose.then_some(stdout.to_string()),
        };
    };

    let unhealthy = items
        .iter()
        .filter(|item| {
            item.get("healthy").and_then(Value::as_bool) == Some(false)
                || item.get("health").and_then(Value::as_str) == Some("unhealthy")
        })
        .count();
    Check {
        name: format!("connector-manager:{endpoint}"),
        status: if unhealthy == 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::Warning
        },
        message: format!("{} item(s), {} unhealthy", items.len(), unhealthy),
        details: verbose.then_some(stdout.to_string()),
    }
}

fn check_postgres_syncs(deployment: &Deployment) -> Vec<Check> {
    let user = deployment
        .env
        .value("DATABASE_USERNAME")
        .unwrap_or_else(|| "omni".into());
    let db = deployment
        .env
        .value("DATABASE_NAME")
        .unwrap_or_else(|| "omni".into());
    let query = "SELECT status, COUNT(*) FROM sync_runs WHERE started_at > NOW() - INTERVAL '24 hours' GROUP BY status ORDER BY status";
    match deployment.compose_output([
        "exec", "-T", "postgres", "psql", "-U", &user, "-d", &db, "-tAc", query,
    ]) {
        Ok(result) if result.success => vec![Check {
            name: "postgres:sync-runs".into(),
            status: CheckStatus::Ok,
            message: "recent sync_runs query succeeded".into(),
            details: Some(result.stdout.trim().to_string()),
        }],
        Ok(result) => vec![Check {
            name: "postgres:sync-runs".into(),
            status: CheckStatus::Warning,
            message: "could not query recent sync_runs".into(),
            details: Some(trim_details(result.stderr, result.stdout)),
        }],
        Err(error) => vec![Check {
            name: "postgres:sync-runs".into(),
            status: CheckStatus::Warning,
            message: error.to_string(),
            details: None,
        }],
    }
}

fn check_logs(deployment: &Deployment, logs_since: &str) -> Vec<Check> {
    match deployment.compose_output(["logs", "--since", logs_since, "--tail", "500"]) {
        Ok(result) if result.success => {
            let lower = result.stdout.to_ascii_lowercase();
            let hits = [
                "error",
                "panic",
                "failed migration",
                "unhealthy",
                "restarting",
            ]
            .into_iter()
            .filter(|needle| lower.contains(needle))
            .collect::<Vec<_>>();
            vec![Check {
                name: "logs:recent".into(),
                status: if hits.is_empty() {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                message: if hits.is_empty() {
                    format!("no obvious errors in logs since {logs_since}")
                } else {
                    format!("found suspicious log terms: {}", hits.join(", "))
                },
                details: if hits.is_empty() {
                    None
                } else {
                    Some(sample_matching_lines(&result.stdout))
                },
            }]
        }
        Ok(result) => vec![command_check("logs:recent", result)],
        Err(error) => vec![Check {
            name: "logs:recent".into(),
            status: CheckStatus::Warning,
            message: error.to_string(),
            details: None,
        }],
    }
}

fn command_check(prefix: &str, result: CommandResult) -> Check {
    Check {
        name: format!("{prefix}:{}", result.command),
        status: if result.success {
            CheckStatus::Ok
        } else {
            CheckStatus::Error
        },
        message: if result.success {
            result
                .stdout
                .lines()
                .next()
                .unwrap_or("command succeeded")
                .to_string()
        } else {
            format!("command failed with status {:?}", result.status)
        },
        details: (!result.stderr.trim().is_empty()).then_some(result.stderr),
    }
}

fn text_field(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn trim_details(stderr: String, stdout: String) -> String {
    let mut details = String::new();
    if !stderr.trim().is_empty() {
        details.push_str(stderr.trim());
    }
    if !stdout.trim().is_empty() {
        if !details.is_empty() {
            details.push('\n');
        }
        details.push_str(stdout.trim());
    }
    details
}

fn sample_matching_lines(logs: &str) -> String {
    logs.lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            ["error", "panic", "failed", "unhealthy", "restarting"]
                .iter()
                .any(|needle| lower.contains(needle))
        })
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_compose_ps() {
        let checks =
            parse_compose_ps(r#"[{"Service":"web","State":"running","Health":"healthy"}]"#);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn parses_line_compose_ps() {
        let checks = parse_compose_ps("{\"Service\":\"web\",\"State\":\"exited\"}\n");
        assert_eq!(checks[0].status, CheckStatus::Warning);
    }
}
