use crate::diagnostics::{CheckStatus, DoctorReport};
use crate::managed_files::FileChange;
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, Color, ContentArrangement, Table};

pub fn print_doctor_report(report: &DoctorReport) {
    let (ok, warnings, errors, info) = report.summary_counts();
    println!("Omni doctor for {}", report.install_dir);
    if let Some(version) = &report.omni_version {
        println!("OMNI_VERSION={version}");
    }
    println!("Summary: {ok} ok, {warnings} warning(s), {errors} error(s), {info} info\n");

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["Status", "Check", "Message"]);
    for check in &report.checks {
        table.add_row(vec![
            status_cell(&check.status),
            Cell::new(&check.name).add_attribute(Attribute::Bold),
            Cell::new(&check.message),
        ]);
        if let Some(details) = &check.details {
            if !details.trim().is_empty() {
                table.add_row(vec![
                    Cell::new(""),
                    Cell::new("details"),
                    Cell::new(details),
                ]);
            }
        }
    }
    println!("{table}");
}

pub fn print_file_changes(changes: &[FileChange]) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["File", "Change", "Local edit"]);
    for change in changes {
        table.add_row(vec![
            Cell::new(&change.path),
            Cell::new(if change.changed {
                "replace/update"
            } else {
                "unchanged"
            }),
            Cell::new(if change.local_edit_detected {
                "yes"
            } else {
                "no"
            }),
        ]);
    }
    println!("{table}");
}

fn status_cell(status: &CheckStatus) -> Cell {
    match status {
        CheckStatus::Ok => Cell::new("ok").fg(Color::Green),
        CheckStatus::Warning => Cell::new("warn").fg(Color::Yellow),
        CheckStatus::Error => Cell::new("error").fg(Color::Red),
        CheckStatus::Info => Cell::new("info").fg(Color::Blue),
    }
}
