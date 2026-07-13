use crate::compose::{check_docker_available, Deployment};
use crate::diagnostics;
use crate::env_file::{image_tag_from_release_tag, EnvFile};
use crate::managed_files;
use crate::output;
use crate::releases::{self, DOCKER_COMPOSE_ASSET};
use crate::UpgradeArgs;
use anyhow::{bail, Context, Result};
use inquire::{Confirm, Text};
use std::path::{Path, PathBuf};

pub async fn run(args: UpgradeArgs) -> Result<()> {
    let deployment = Deployment::discover(args.install.install_dir.clone())?;
    preflight()?;

    let current_version = deployment
        .env
        .value("OMNI_VERSION")
        .unwrap_or_else(|| "latest".into());
    let release = releases::resolve_release(args.to.as_deref()).await?;
    let image_tag = image_tag_from_release_tag(&release.tag_name);
    release.asset(DOCKER_COMPOSE_ASSET)?;

    println!(
        "Preparing Omni upgrade: {} -> {} (image tag {})",
        current_version, release.tag_name, image_tag
    );

    let temp = tempfile::tempdir().context("failed to create temporary directory")?;
    let archive_path = temp.path().join(DOCKER_COMPOSE_ASSET);
    let extract_dir = temp.path().join("release");

    if args.dry_run {
        println!("dry run: downloading release asset for diff only");
    }
    releases::download_asset_verified(&release, DOCKER_COMPOSE_ASSET, &archive_path).await?;
    releases::extract_docker_compose_archive(&archive_path, &extract_dir)?;

    let mut file_changes = managed_files::analyze(&deployment.root, &extract_dir)?;
    apply_first_upgrade_local_edit_detection(
        &deployment,
        &current_version,
        &release.tag_name,
        &extract_dir,
        temp.path(),
        &mut file_changes,
    )
    .await?;
    let target_env_example = extract_dir.join(".env.example");
    let env_plan = build_env_plan(&deployment.env_file, &target_env_example, &image_tag)?;

    println!("\nManaged file changes:");
    output::print_file_changes(&file_changes);
    print_env_plan(&env_plan);

    let local_edits: Vec<_> = file_changes
        .iter()
        .filter(|change| change.local_edit_detected)
        .collect();
    if !local_edits.is_empty() && !args.force {
        println!("\nLocal edits were detected in managed files:");
        for change in local_edits {
            println!("  - {}", change.path);
        }
        println!("Use --force to continue after reviewing the backup/override-file guidance.");
        bail!("refusing to overwrite locally edited managed files without --force");
    }

    if args.dry_run {
        println!("\nDry run complete. No files were changed and Docker Compose was not run.");
        return Ok(());
    }

    if !args.yes && !confirm("Proceed with the upgrade?", false)? {
        println!("Upgrade cancelled.");
        return Ok(());
    }

    let backup_dir = managed_files::create_backup(&deployment.root, &[".env"])?;
    println!("Created backup at {}", backup_dir.display());

    apply_env_plan(&deployment.env_file, env_plan, args.yes)?;
    managed_files::replace_managed_files(&deployment.root, &extract_dir, false)?;

    if !args.skip_pull {
        println!("\nPulling images...");
        deployment.compose_stream(["pull"])?;
    }

    if !args.skip_up {
        println!("\nRecreating services...");
        deployment.compose_stream(["up", "-d", "--remove-orphans"])?;
    }

    println!("\nRunning post-upgrade doctor summary...");
    let refreshed = Deployment::discover(Some(deployment.root.clone()))?;
    let report = diagnostics::run_doctor(&refreshed, false, "10m").await;
    output::print_doctor_report(&report);

    println!("\nUpgrade complete.");
    println!("Rollback files are in {}", backup_dir.display());
    println!(
        "To roll back manually, restore files from that directory, ensure OMNI_VERSION={} in .env, then run:\n  {}\n  {}",
        current_version,
        refreshed.compose_command_string(["pull"]),
        refreshed.compose_command_string(["up", "-d", "--remove-orphans"]),
    );

    Ok(())
}

fn preflight() -> Result<()> {
    let results = check_docker_available()?;
    for result in results {
        if !result.success {
            bail!("{} failed: {}", result.command, result.stderr.trim());
        }
    }
    Ok(())
}

async fn apply_first_upgrade_local_edit_detection(
    deployment: &Deployment,
    current_version: &str,
    target_release_tag: &str,
    target_extract_dir: &Path,
    temp_root: &Path,
    file_changes: &mut [managed_files::FileChange],
) -> Result<()> {
    if managed_files::manifest_exists(&deployment.root) {
        return Ok(());
    }

    if current_version == "latest" || current_version == target_release_tag {
        println!(
            "No previous managed-file manifest found; treating changed existing managed files as local edits."
        );
        managed_files::mark_changed_existing_files_as_local_edits(file_changes);
        return Ok(());
    }

    let current_release = match releases::resolve_release(Some(current_version)).await {
        Ok(release) => release,
        Err(error) => {
            eprintln!(
                "warning: could not fetch current release {current_version} for first-upgrade local edit detection: {error}"
            );
            managed_files::mark_changed_existing_files_as_local_edits(file_changes);
            return Ok(());
        }
    };

    if current_release.tag_name == target_release_tag {
        managed_files::mark_local_edits_against_base(
            file_changes,
            &deployment.root,
            target_extract_dir,
        )?;
        return Ok(());
    }

    let current_archive_path = temp_root.join("current-omni-docker-compose.tar.gz");
    let current_extract_dir = temp_root.join("current-release");
    if let Err(error) = releases::download_asset_verified(
        &current_release,
        DOCKER_COMPOSE_ASSET,
        &current_archive_path,
    )
    .await
    .and_then(|_| {
        releases::extract_docker_compose_archive(&current_archive_path, &current_extract_dir)
    }) {
        eprintln!(
            "warning: could not inspect current release {} for first-upgrade local edit detection: {error}",
            current_release.tag_name
        );
        managed_files::mark_changed_existing_files_as_local_edits(file_changes);
        return Ok(());
    }

    managed_files::mark_local_edits_against_base(
        file_changes,
        &deployment.root,
        &current_extract_dir,
    )?;
    Ok(())
}

#[derive(Debug)]
struct EnvPlan {
    image_tag: String,
    missing: Vec<(String, String)>,
    removed: Vec<String>,
}

fn build_env_plan(env_path: &Path, target_env_example: &Path, image_tag: &str) -> Result<EnvPlan> {
    let local = EnvFile::load(env_path)?;
    let target = EnvFile::load(target_env_example).with_context(|| {
        format!(
            "release asset did not include expected {}",
            target_env_example.display()
        )
    })?;
    let diff = local.diff_against_template(&target);
    let missing = diff
        .missing
        .into_iter()
        .map(|key| {
            let default = target.raw_value(&key).unwrap_or_default();
            (key, default)
        })
        .collect();
    Ok(EnvPlan {
        image_tag: image_tag.to_string(),
        missing,
        removed: diff.removed,
    })
}

fn print_env_plan(plan: &EnvPlan) {
    println!("\n.env changes:");
    println!("  - set OMNI_VERSION={}", plan.image_tag);
    if plan.missing.is_empty() {
        println!("  - no missing variables from target .env.example");
    } else {
        println!("  - missing variables to review/add:");
        for (key, default) in &plan.missing {
            if default.is_empty() {
                println!("    {key}");
            } else {
                println!("    {key}={default}");
            }
        }
    }
    if !plan.removed.is_empty() {
        println!("  - variables no longer present in target template (left untouched):");
        for key in &plan.removed {
            println!("    {key}");
        }
    }
}

fn apply_env_plan(env_path: &Path, plan: EnvPlan, yes: bool) -> Result<()> {
    let mut env = EnvFile::load(env_path)?;
    env.set("OMNI_VERSION", plan.image_tag);

    let mut additions = Vec::new();
    if !plan.missing.is_empty() {
        println!("\nNew/missing env vars found. They are warnings, not hard errors.");
        let should_add = yes || confirm("Append missing variables to .env now?", true)?;
        if should_add {
            for (key, default) in plan.missing {
                let value = if yes {
                    default
                } else {
                    Text::new(&format!("Value for {key}"))
                        .with_default(&default)
                        .prompt()
                        .unwrap_or(default)
                };
                additions.push((key, value));
            }
        }
    }
    env.append_section("Added by omni upgrade", &additions);
    env.save(env_path)?;
    Ok(())
}

fn confirm(message: &str, default: bool) -> Result<bool> {
    Ok(Confirm::new(message).with_default(default).prompt()?)
}

#[allow(dead_code)]
fn _assert_paths(_: PathBuf) {}
