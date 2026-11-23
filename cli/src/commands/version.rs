use crate::compose::Deployment;
use crate::releases;
use crate::VersionArgs;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
struct VersionReport {
    cli_version: &'static str,
    configured_omni_version: Option<String>,
    running_omni_image_tags: Vec<String>,
    install_dir: Option<String>,
    warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    update_available: Option<Updates>,
}

#[derive(Debug, Serialize)]
struct Updates {
    latest_release: String,
    cli_outdated: bool,
    omni_deployment_outdated: bool,
    cli_install_command: &'static str,
    omni_upgrade_command: &'static str,
}

pub async fn run(args: VersionArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir.clone())?;
    let configured_version = deployment
        .env
        .value("OMNI_VERSION")
        .unwrap_or_else(|| "latest".into());

    let updates = if args.check {
        check_updates(&configured_version).await
    } else {
        None
    };

    let report = build_report_from_deployment(&deployment);
    if args.json {
        let mut report_json = serde_json::to_value(&report)?;
        if let Some(updates) = &updates {
            if let Some(obj) = report_json.as_object_mut() {
                obj.insert(
                    "update_available".to_string(),
                    serde_json::to_value(updates).unwrap_or_default(),
                );
            }
        }
        println!("{}", serde_json::to_string_pretty(&report_json)?);
    } else {
        println!("Omni CLI: v{}", report.cli_version);
        if let Some(install_dir) = &report.install_dir {
            println!("Install dir: {install_dir}");
        }
        if let Some(configured) = &report.configured_omni_version {
            println!("Configured Omni version: {configured}");
        }
        if report.running_omni_image_tags.is_empty() {
            println!("Running Omni image tags: unavailable");
        } else {
            println!(
                "Running Omni image tags: {}",
                report.running_omni_image_tags.join(", ")
            );
        }
        if let Some(warning) = &report.warning {
            println!("Warning: {warning}");
        }

        if let Some(updates) = &updates {
            let mut any_outdated = false;

            if updates.cli_outdated {
                any_outdated = true;
                println!();
                println!(
                    "A new CLI version is available: {} (current: v{})",
                    updates.latest_release, report.cli_version
                );
                println!("{}", updates.cli_install_command);
            }

            if updates.omni_deployment_outdated {
                any_outdated = true;
                println!();
                println!(
                    "A new Omni release is available: {} (current: {})",
                    updates.latest_release, configured_version
                );
                println!("{}", updates.omni_upgrade_command);
            }

            if !any_outdated {
                println!();
                println!(
                    "Omni CLI and deployment are up to date at v{}.",
                    report.cli_version
                );
            }
        }
    }
    Ok(())
}

async fn check_updates(configured_version: &str) -> Option<Updates> {
    let release = match releases::latest_stable_release(
        &reqwest::Client::builder()
            .user_agent(format!("omni-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .ok()?,
        releases::DEFAULT_REPO,
    )
    .await
    {
        Ok(release) => release,
        Err(err) => {
            eprintln!("warning: could not check for updates: {err}");
            return None;
        }
    };

    let cli_version = format!("v{}", env!("CARGO_PKG_VERSION"));
    let cli_outdated =
        releases::compare_versions(&cli_version, &release.tag_name) == Ordering::Less;

    let omni_deployment_outdated = configured_version != "latest"
        && releases::compare_versions(configured_version, &release.tag_name) == Ordering::Less;

    Some(Updates {
        latest_release: release.tag_name,
        cli_outdated,
        omni_deployment_outdated,
        cli_install_command: "Run: curl -fsSL https://github.com/getomnico/omni/releases/latest/download/install-cli.sh | sh",
        omni_upgrade_command: "Run: omni upgrade",
    })
}

fn build_report_from_deployment(deployment: &Deployment) -> VersionReport {
    let running_omni_image_tags = running_omni_image_tags(deployment).unwrap_or_default();
    VersionReport {
        cli_version: env!("CARGO_PKG_VERSION"),
        configured_omni_version: deployment.env.value("OMNI_VERSION"),
        running_omni_image_tags,
        install_dir: Some(deployment.root.display().to_string()),
        warning: None,
        update_available: None,
    }
}

fn running_omni_image_tags(deployment: &Deployment) -> Result<Vec<String>> {
    let result = deployment.compose_output(["ps", "--format", "json"])?;
    if !result.success {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    if let Ok(Value::Array(array)) = serde_json::from_str::<Value>(&result.stdout) {
        values = array;
    } else {
        for line in result.stdout.lines().filter(|line| !line.trim().is_empty()) {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                values.push(value);
            }
        }
    }

    let mut tags = BTreeSet::new();
    for value in values {
        if let Some(image) = value.get("Image").and_then(Value::as_str) {
            if image.contains("ghcr.io/getomnico/omni/") {
                if let Some((_, tag)) = image.rsplit_once(':') {
                    tags.insert(tag.to_string());
                }
            }
        }
    }
    Ok(tags.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_report_compiles() {
        // Build report tested through integration tests.
    }
}
