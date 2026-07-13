use crate::compose::Deployment;
use crate::env_file::EnvFile;
use crate::releases::{self, DOCKER_COMPOSE_ASSET};
use crate::{EnvArgs, EnvCommand};
use anyhow::{Context, Result};

pub async fn run(args: EnvArgs) -> Result<()> {
    match args.command {
        EnvCommand::Diff(args) => diff(args).await,
    }
}

async fn diff(args: crate::EnvDiffArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir)?;
    let release = releases::resolve_release(args.to.as_deref()).await?;
    let temp = tempfile::tempdir().context("failed to create temporary directory")?;
    let archive_path = temp.path().join(DOCKER_COMPOSE_ASSET);
    let extract_dir = temp.path().join("release");
    releases::download_asset_verified(&release, DOCKER_COMPOSE_ASSET, &archive_path).await?;
    releases::extract_docker_compose_archive(&archive_path, &extract_dir)?;

    let local = EnvFile::load(&deployment.env_file)?;
    let target = EnvFile::load(&extract_dir.join(".env.example"))?;
    let diff = local.diff_against_template(&target);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        println!("Environment diff against {}:", release.tag_name);
        if diff.missing.is_empty() {
            println!("No variables missing from local .env.");
        } else {
            println!("\nMissing locally (warning only):");
            for key in diff.missing {
                println!("  {key}");
            }
        }
        if !diff.removed.is_empty() {
            println!("\nOnly in local .env / removed from target template:");
            for key in diff.removed {
                println!("  {key}");
            }
        }
    }
    Ok(())
}
