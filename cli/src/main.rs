mod commands;
mod compose;
mod diagnostics;
mod env_file;
mod managed_files;
mod output;
mod releases;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "omni",
    version,
    about = "Manage Omni Docker Compose deployments",
    long_about = "A CLI for upgrading and diagnosing Omni Docker Compose deployments."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Upgrade an Omni Docker Compose deployment.
    Upgrade(UpgradeArgs),
    /// Run deployment diagnostics.
    Doctor(DoctorArgs),
    /// Environment file helpers.
    Env(EnvArgs),
    /// Show a concise service status summary.
    Status(StatusArgs),
    /// Show Docker Compose logs.
    Logs(LogsArgs),
    /// Pull Omni Docker Compose images.
    Pull(PullArgs),
    /// Create or update Omni Docker Compose services.
    Up(UpArgs),
    /// Back up managed deployment files without upgrading.
    Backup(BackupArgs),
    /// Print CLI and deployment version information.
    Version(VersionArgs),
    /// Run docker compose with Omni's deployment files and .env.
    Compose(ComposeArgs),
}

#[derive(Debug, Args, Clone)]
struct InstallDirArg {
    /// Path to the Omni Docker Compose install directory.
    #[arg(long, value_name = "PATH", global = false)]
    install_dir: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
struct UpgradeArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Target Omni release. Defaults to the latest stable GitHub release.
    #[arg(long = "to", value_name = "VERSION")]
    to: Option<String>,
    /// Show what would change without writing files or running Docker Compose.
    #[arg(long)]
    dry_run: bool,
    /// Accept prompts using safe defaults.
    #[arg(short = 'y', long)]
    yes: bool,
    /// Continue even when local edits to managed files are detected.
    #[arg(long)]
    force: bool,
    /// Do not run docker compose pull.
    #[arg(long)]
    skip_pull: bool,
    /// Do not run docker compose up.
    #[arg(long)]
    skip_up: bool,
}

#[derive(Debug, Args, Clone)]
struct DoctorArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
    /// Include extra checks and command output details.
    #[arg(long, short)]
    verbose: bool,
    /// Time window for log scanning, e.g. 30m, 2h, 1d.
    #[arg(long, default_value = "30m")]
    logs_since: String,
}

#[derive(Debug, Args, Clone)]
struct StatusArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args, Clone)]
struct LogsArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Optional service name, such as web, postgres, or connector-manager.
    service: Option<String>,
    /// Follow log output.
    #[arg(short, long)]
    follow: bool,
    /// Number of log lines to show.
    #[arg(long, default_value_t = 200)]
    tail: u32,
}

#[derive(Debug, Args, Clone)]
struct PullArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Optional services to pull. Defaults to all services in the Compose project.
    services: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct UpArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Optional services to create/update. Defaults to the whole Compose project.
    services: Vec<String>,
    /// Run in the foreground instead of detached mode.
    #[arg(long)]
    no_detach: bool,
    /// Do not remove containers for services no longer in the Compose file.
    #[arg(long)]
    no_remove_orphans: bool,
}

#[derive(Debug, Args, Clone)]
struct BackupArgs {
    #[command(flatten)]
    install: InstallDirArg,
}

#[derive(Debug, Args, Clone)]
struct ComposeArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Arguments to pass after `docker compose`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct VersionArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args, Clone)]
struct EnvArgs {
    #[command(subcommand)]
    command: EnvCommand,
}

#[derive(Debug, Subcommand, Clone)]
enum EnvCommand {
    /// Preview environment variables added to or removed from a release template.
    Diff(EnvDiffArgs),
}

#[derive(Debug, Args, Clone)]
struct EnvDiffArgs {
    #[command(flatten)]
    install: InstallDirArg,
    /// Target Omni release. Defaults to the latest stable GitHub release.
    #[arg(long = "to", value_name = "VERSION")]
    to: Option<String>,
    /// Emit machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Upgrade(args) => commands::upgrade::run(args).await,
        Command::Doctor(args) => commands::doctor::run(args).await,
        Command::Env(args) => commands::env::run(args).await,
        Command::Status(args) => commands::status::run(args).await,
        Command::Logs(args) => commands::logs::run(args).await,
        Command::Pull(args) => commands::pull::run(args).await,
        Command::Up(args) => commands::up::run(args).await,
        Command::Backup(args) => commands::backup::run(args).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Compose(args) => commands::compose_passthrough::run(args).await,
    }
}
