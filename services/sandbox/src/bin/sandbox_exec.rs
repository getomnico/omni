//! Landlock sandbox executor — restricts filesystem access then exec's user command.
//!
//! Usage: sandbox-exec <chat_dir> -- <command...>
//!
//! Uses Linux Landlock (kernel 5.13+) to restrict the calling process and all
//! its children to a minimal set of filesystem paths before exec'ing the
//! user-supplied command.

use std::ffi::CString;
use std::path::Path;

use nix::libc;

use landlock::{
    path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("sandbox-exec: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Parse: sandbox-exec <chat_dir> -- <command...>
    let sep = args
        .iter()
        .position(|a| a == "--")
        .ok_or("Usage: sandbox-exec <chat_dir> -- <command...>")?;

    if sep < 2 {
        return Err("Usage: sandbox-exec <chat_dir> -- <command...>".into());
    }

    let chat_dir = &args[1];
    let command = &args[sep + 1..];

    if command.is_empty() {
        return Err("No command specified after --".into());
    }

    if !Path::new(chat_dir).is_dir() {
        return Err(format!("Chat directory does not exist: {chat_dir}").into());
    }

    // 1. Set no-new-privileges (required by Landlock)
    // Safety: prctl with PR_SET_NO_NEW_PRIVS is a simple flag set
    let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ret != 0 {
        return Err("prctl(PR_SET_NO_NEW_PRIVS) failed".into());
    }

    // 2. Build Landlock ruleset
    let read_access = AccessFs::Execute | AccessFs::ReadFile | AccessFs::ReadDir;
    let all_access = AccessFs::from_all(ABI::V3);

    let read_only_paths: Vec<&str> = ["/usr", "/lib", "/bin", "/etc", "/lib64"]
        .iter()
        .copied()
        .filter(|p| Path::new(p).exists())
        .collect();

    let rw_paths: Vec<&str> = [chat_dir.as_str(), "/tmp", "/dev"]
        .iter()
        .copied()
        .filter(|p| Path::new(p).exists())
        .collect();

    Ruleset::default()
        .handle_access(all_access)?
        .create()?
        .add_rules(path_beneath_rules(&read_only_paths, read_access))?
        .add_rules(path_beneath_rules(&rw_paths, all_access))?
        .restrict_self()?;

    // 3. Set up environment and exec
    std::env::set_current_dir(chat_dir)?;
    std::env::set_var("HOME", chat_dir);
    std::env::set_var("TMPDIR", "/tmp");
    std::env::set_var("PATH", "/usr/local/bin:/usr/bin:/bin");
    std::env::set_var("PYTHONDONTWRITEBYTECODE", "1");

    // 4. execvp
    let program = CString::new(command[0].as_str())?;
    let c_args: Vec<CString> = command
        .iter()
        .map(|a| CString::new(a.as_str()).unwrap())
        .collect();

    nix::unistd::execvp(&program, &c_args)?;

    // execvp never returns on success
    unreachable!()
}
