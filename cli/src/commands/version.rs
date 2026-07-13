use crate::compose::Deployment;
use crate::VersionArgs;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
struct VersionReport {
    cli_version: &'static str,
    configured_omni_version: Option<String>,
    running_omni_image_tags: Vec<String>,
    install_dir: Option<String>,
    warning: Option<String>,
}

pub async fn run(args: VersionArgs) -> Result<()> {
    let report = build_report(args.install.install_dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Omni CLI: {}", report.cli_version);
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
    }
    Ok(())
}

fn build_report(install_dir: Option<std::path::PathBuf>) -> Result<VersionReport> {
    let deployment = Deployment::discover(install_dir)?;
    let running_omni_image_tags = running_omni_image_tags(&deployment).unwrap_or_default();
    Ok(VersionReport {
        cli_version: env!("CARGO_PKG_VERSION"),
        configured_omni_version: deployment.env.value("OMNI_VERSION"),
        running_omni_image_tags,
        install_dir: Some(deployment.root.display().to_string()),
        warning: None,
    })
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
    fn version_report_requires_install_dir() {
        let report = build_report(Some(std::path::PathBuf::from("/definitely/missing")));
        assert!(report.is_err());

        let report = build_report(None);
        assert!(report.is_err());
    }
}
