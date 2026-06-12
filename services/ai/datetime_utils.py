from datetime import UTC, datetime
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from db.models import UserConfiguration


def resolve_zone(user_configuration: UserConfiguration | None = None) -> ZoneInfo:
    timezone = user_configuration.timezone if user_configuration else None
    if not timezone:
        return ZoneInfo("UTC")
    try:
        return ZoneInfo(timezone)
    except ZoneInfoNotFoundError:
        return ZoneInfo("UTC")


def format_datetime(
    dt: datetime | None = None,
    user_configuration: UserConfiguration | None = None,
    fmt: str = "%A, %B %d, %Y %H:%M {zone}",
) -> str:
    zone = resolve_zone(user_configuration)
    if dt is None:
        dt = datetime.now(UTC)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=UTC)
    local_dt = dt.astimezone(zone)
    zone_label = getattr(zone, "key", "UTC")
    return local_dt.strftime(fmt.format(zone=zone_label))


def format_search_date(dt: datetime, user_configuration: UserConfiguration | None = None) -> str:
    return format_datetime(dt, user_configuration, fmt="%Y-%m-%d %H:%M {zone}")
