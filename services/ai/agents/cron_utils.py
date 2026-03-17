"""Schedule computation utilities for cron and interval schedules."""

from datetime import datetime, timedelta, timezone

from croniter import croniter

from .models import ScheduleType


def compute_next_run(
    schedule_type: ScheduleType,
    schedule_value: str,
    from_time: datetime,
) -> datetime:
    """Compute the next run time from a given base time.

    Args:
        schedule_type: "cron" or "interval"
        schedule_value: cron expression or interval in seconds
        from_time: base time to compute next run from
    """
    if schedule_type == "cron":
        cron = croniter(schedule_value, from_time)
        return cron.get_next(datetime)
    elif schedule_type == "interval":
        seconds = int(schedule_value)
        return from_time + timedelta(seconds=seconds)
    else:
        raise ValueError(f"Unknown schedule_type: {schedule_type}")


def validate_schedule(schedule_type: ScheduleType, schedule_value: str) -> bool:
    """Validate a schedule configuration."""
    if schedule_type == "cron":
        return croniter.is_valid(schedule_value)
    elif schedule_type == "interval":
        try:
            seconds = int(schedule_value)
            return seconds > 0
        except (ValueError, TypeError):
            return False
    return False


def is_due(
    schedule_type: ScheduleType,
    schedule_value: str,
    last_run_time: datetime,
    now: datetime,
) -> bool:
    """Check if an agent is due for execution."""
    next_run = compute_next_run(schedule_type, schedule_value, last_run_time)
    return now >= next_run
