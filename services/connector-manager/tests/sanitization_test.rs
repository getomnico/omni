/// Sanitization regression tests for connector-manager logs.
///
/// These tests verify that no raw IDs, response bodies, connector URLs,
/// or user identifiers appear in log messages.  They scan the actual
/// source files, not runtime behaviour.
///
/// NOTE: functional non-log code is excluded via a "log-only" heuristic:
/// we only match lines that contain one of the recognised log macros
/// (info!, warn!, error!, debug!, trace!).
use std::fs;
use std::path::Path;

/// Check that a file path is a Rust source file inside the src/ directory.
fn is_rust_source(path: &Path) -> bool {
    path.extension().map(|e| e == "rs").unwrap_or(false)
        && path.components().any(|c| c.as_os_str() == "src")
}

/// Check that a line appears to be a log macro invocation (or a continuation line
/// of one) rather than functional non-log code.
fn is_log_or_continuation(line: &str, in_log_call: &mut bool) -> bool {
    let trimmed = line.trim();
    if *in_log_call {
        // Keep scanning as continuation if it ends with a comma or is a string
        // continuation. Stop when we see a closing paren or semicolon.
        if trimmed.starts_with(')') || trimmed.starts_with("};") || trimmed.ends_with(';') {
            *in_log_call = false;
            return true;
        }
        return true;
    }
    if trimmed.starts_with("info!(")
        || trimmed.starts_with("warn!(")
        || trimmed.starts_with("error!(")
        || trimmed.starts_with("debug!(")
        || trimmed.starts_with("trace!(")
    {
        *in_log_call = !trimmed.contains(';') && !trimmed.ends_with(')');
        return true;
    }
    false
}

/// Scan a single file for forbidden raw-value patterns in log lines only.
/// Returns a list of (line_number, line_text) for matches.
fn scan_log_forbidden(path: &Path, forbidden_patterns: &[&str]) -> Vec<(usize, String)> {
    let content = fs::read_to_string(path).unwrap();
    let mut hits = Vec::new();
    let mut in_log_call = false;

    for (i, line) in content.lines().enumerate() {
        if !is_log_or_continuation(line, &mut in_log_call) {
            continue;
        }
        // Extend continuation tracking: check for open paren without matching close
        // on the same line.
        let trimmed = line.trim();
        if !in_log_call && trimmed.starts_with("info!(")
            || trimmed.starts_with("warn!(")
            || trimmed.starts_with("error!(")
            || trimmed.starts_with("debug!(")
        {
            let has_close = trimmed
                .rfind(')')
                .map(|p| {
                    // Count parens
                    let opens = trimmed[..p].matches('(').count();
                    let closes = trimmed[..p].matches(')').count();
                    opens == closes + 1
                })
                .unwrap_or(false);
            if !has_close {
                in_log_call = true;
            }
        }

        for pat in forbidden_patterns {
            if line.contains(pat) {
                hits.push((i + 1, line.to_string()));
                break;
            }
        }
    }

    hits
}

const FORBIDDEN_CONNECTOR_CLIENT: &[&str] = &[
    "response.text()", // reading full response body
    "body",            // response body variable in error log
    "{} - {}",         // pattern with status + body (in format args)
    "connector_url",   // raw connector URL in log
    "error = %e",      // error display may embed URL/path/body
];

const FORBIDDEN_LOGS: &[&str] = &[
    "source_id",     // source identifier in log message
    "sync_run_id",   // sync run identifier in log message
    "user_email",    // user email in log message
    ".email",        // email field access in log
    ".id",           // generic ID field access in log
    "connector_url", // raw URL in log message
];

const FORBIDDEN_HANDLERS_SYNC: &[&str] = &[
    "request.source_id",   // source ID as format param in log call
    "request.sync_run_id", // sync run ID as format param
    "fields.sync_run_id",  // sync run ID in extract handler
    "source_id",           // source variable in log message
    "sync_run_id",         // sync run variable in log message
];

fn resolve_path(name: &str) -> std::path::PathBuf {
    let candidates = vec![
        Path::new(name).to_path_buf(),
        Path::new("services/connector-manager").join(name),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    panic!("{} not found in any candidate path", name);
}

#[test]
fn test_handlers_sanitization() {
    let path = resolve_path("src/handlers.rs");
    let hits = scan_log_forbidden(&path, FORBIDDEN_HANDLERS_SYNC);
    assert!(
        hits.is_empty(),
        "Found forbidden patterns in handlers.rs log lines:\n{}",
        hits.iter()
            .map(|(n, l)| format!("  line {}: {}", n, l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn test_sync_manager_sanitization() {
    let path = resolve_path("src/sync_manager.rs");
    let hits = scan_log_forbidden(&path, FORBIDDEN_HANDLERS_SYNC);
    assert!(
        hits.is_empty(),
        "Found forbidden patterns in sync_manager.rs log lines:\n{}",
        hits.iter()
            .map(|(n, l)| format!("  line {}: {}", n, l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn test_connector_client_sanitization() {
    let path = resolve_path("src/connector_client.rs");
    let hits = scan_log_forbidden(&path, FORBIDDEN_CONNECTOR_CLIENT);
    assert!(
        hits.is_empty(),
        "Found forbidden patterns in connector_client.rs log lines:\n{}",
        hits.iter()
            .map(|(n, l)| format!("  line {}: {}", n, l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
