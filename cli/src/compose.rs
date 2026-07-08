use crate::env_file::EnvFile;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Clone)]
pub struct Deployment {
    pub root: PathBuf,
    pub env_file: PathBuf,
    pub env_example: PathBuf,
    pub compose_file: PathBuf,
    pub local_inference_compose_file: PathBuf,
    pub caddyfile: PathBuf,
    pub overrides: Vec<PathBuf>,
    pub env: EnvFile,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub command: String,
    pub success: bool,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Deployment {
    pub fn discover(install_dir: Option<PathBuf>) -> Result<Self> {
        let root = match install_dir {
            Some(path) => path,
            None => find_install_dir(std::env::current_dir().context("failed to read cwd")?)?,
        };
        let env_file = root.join(".env");
        let env_example = root.join(".env.example");
        let compose_file = root.join("docker/docker-compose.yml");
        let local_inference_compose_file = root.join("docker/docker-compose.local-inference.yml");
        let caddyfile = root.join("Caddyfile");

        if !env_file.exists() {
            bail!(
                "{} not found; run from an Omni Docker Compose install directory or pass --install-dir",
                env_file.display()
            );
        }
        if !compose_file.exists() {
            bail!(
                "{} not found; only Docker Compose deployments are supported",
                compose_file.display()
            );
        }

        let env = EnvFile::load(&env_file)?;
        let overrides = discover_overrides(&root);

        Ok(Self {
            root,
            env_file,
            env_example,
            compose_file,
            local_inference_compose_file,
            caddyfile,
            overrides,
            env,
        })
    }

    pub fn compose_base_args(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("compose"),
            OsString::from("--env-file"),
            OsString::from(".env"),
            OsString::from("-f"),
            OsString::from("docker/docker-compose.yml"),
        ];
        for override_file in &self.overrides {
            if let Ok(relative) = override_file.strip_prefix(&self.root) {
                args.push(OsString::from("-f"));
                args.push(relative.as_os_str().to_os_string());
            }
        }
        args
    }

    #[cfg(test)]
    pub fn compose_command_string<I, S>(&self, extra_args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut parts = vec!["docker".to_string()];
        parts.extend(
            self.compose_base_args()
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        parts.extend(
            extra_args
                .into_iter()
                .map(|arg| arg.as_ref().to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }

    pub fn compose_output<I, S>(&self, extra_args: I) -> Result<CommandResult>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let extra: Vec<OsString> = extra_args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let mut args = self.compose_base_args();
        args.extend(extra.clone());
        let output = Command::new("docker")
            .current_dir(&self.root)
            .args(&args)
            .output()
            .with_context(|| "failed to run docker; is Docker installed?")?;
        Ok(command_result("docker", &args, output))
    }

    pub fn compose_stream<I, S>(&self, extra_args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let extra: Vec<OsString> = extra_args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        let mut args = self.compose_base_args();
        args.extend(extra);
        let status = Command::new("docker")
            .current_dir(&self.root)
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| "failed to run docker; is Docker installed?")?;
        if !status.success() {
            bail!(
                "{} failed with status {}",
                format_command("docker", &args),
                status
            );
        }
        Ok(())
    }
}

pub fn check_docker_available() -> Result<Vec<CommandResult>> {
    let mut results = Vec::new();
    results.push(run_simple("docker", ["--version"])?);
    results.push(run_simple("docker", ["compose", "version"])?);
    Ok(results)
}

pub fn run_simple<I, S>(program: &str, args: I) -> Result<CommandResult>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect();
    let output = Command::new(program)
        .args(&args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(command_result(program, &args, output))
}

fn command_result(program: &str, args: &[OsString], output: Output) -> CommandResult {
    CommandResult {
        command: format_command(program, args),
        success: output.status.success(),
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn find_install_dir(start: PathBuf) -> Result<PathBuf> {
    for dir in start.ancestors() {
        if dir.join(".env").exists() && dir.join("docker/docker-compose.yml").exists() {
            return Ok(dir.to_path_buf());
        }
    }
    Err(anyhow!(
        "could not find an Omni Docker Compose install directory from {}",
        start.display()
    ))
}

fn discover_overrides(root: &Path) -> Vec<PathBuf> {
    [
        root.join("docker/docker-compose.override.yml"),
        root.join("docker-compose.override.yml"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn format_command(program: &str, args: &[OsString]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().map(|arg| shell_quote(&arg.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-._/:=".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{escaped}'", escaped = value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::sync::Mutex;

    #[cfg(unix)]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn discovers_install_dir_from_child() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docker")).unwrap();
        fs::write(tmp.path().join(".env"), "OMNI_VERSION=latest\n").unwrap();
        fs::write(
            tmp.path().join("docker/docker-compose.yml"),
            "services: {}\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        let found = find_install_dir(tmp.path().join("a/b")).unwrap();
        assert_eq!(found, tmp.path());
    }

    #[test]
    fn compose_command_uses_env_and_override_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docker")).unwrap();
        fs::write(tmp.path().join(".env"), "OMNI_VERSION=latest\n").unwrap();
        fs::write(
            tmp.path().join("docker/docker-compose.yml"),
            "services: {}\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("docker/docker-compose.override.yml"),
            "services: {}\n",
        )
        .unwrap();
        let dep = Deployment::discover(Some(tmp.path().to_path_buf())).unwrap();
        let command = dep.compose_command_string(["ps"]);
        assert!(command.contains("--env-file .env"));
        assert!(command.contains("docker/docker-compose.override.yml"));
    }

    #[cfg(unix)]
    #[test]
    fn compose_output_uses_mocked_docker_on_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let fake_bin = tmp.path().join("bin");
        fs::create_dir_all(tmp.path().join("install/docker")).unwrap();
        fs::create_dir_all(&fake_bin).unwrap();
        fs::write(tmp.path().join("install/.env"), "OMNI_VERSION=latest\n").unwrap();
        fs::write(
            tmp.path().join("install/docker/docker-compose.yml"),
            "services: {}\n",
        )
        .unwrap();

        let args_file = tmp.path().join("docker-args.txt");
        let docker = fake_bin.join("docker");
        fs::write(
            &docker,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}'\nprintf '{{\"ok\":true}}\\n'\n",
                args_file.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&docker).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&docker, perms).unwrap();

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", fake_bin.display(), old_path.to_string_lossy());
        // SAFETY: This test serializes process environment access with ENV_LOCK
        // and restores PATH before releasing the lock.
        unsafe { std::env::set_var("PATH", new_path) };
        let dep = Deployment::discover(Some(tmp.path().join("install"))).unwrap();
        let result = dep.compose_output(["ps"]).unwrap();
        // SAFETY: See note above.
        unsafe { std::env::set_var("PATH", old_path) };

        assert!(result.success);
        let args = fs::read_to_string(args_file).unwrap();
        assert!(args.contains("compose --env-file .env -f docker/docker-compose.yml ps"));
    }
}
