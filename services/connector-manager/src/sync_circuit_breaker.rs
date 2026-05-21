use crate::models::TriggerType;
use shared::models::{SyncRun, SyncSlotClass, SyncStatus};

pub(crate) fn has_failure_streak(sync_runs: &[SyncRun], max_consecutive_failures: i32) -> bool {
    if max_consecutive_failures <= 0 {
        return true;
    }

    current_unsuccessful_streak(sync_runs).len() >= max_consecutive_failures as usize
}

pub(crate) fn current_unsuccessful_streak(sync_runs: &[SyncRun]) -> Vec<&SyncRun> {
    // Manual failures are ignored because they are user-initiated probes and
    // should not trip scheduled-sync backoff/circuit breaking. Manual successes
    // still break the streak because they prove the source can sync again.
    let mut streak = Vec::new();

    for run in sync_runs
        .iter()
        .filter(|run| run.sync_type.slot_class() == SyncSlotClass::Scheduled)
    {
        match run.status {
            SyncStatus::Completed | SyncStatus::Running => break,
            SyncStatus::Failed | SyncStatus::Cancelled
                if run.trigger_type == TriggerType::Manual.to_string() => {}
            SyncStatus::Failed | SyncStatus::Cancelled => streak.push(run),
        }
    }

    streak
}
