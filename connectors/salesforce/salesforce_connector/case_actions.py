"""Generic Salesforce Case actions backed by provider metadata and sharing."""

# The action schemas and SOQL statements are intentionally explicit; keep their
# long lines readable rather than obscuring field-level security decisions.
# ruff: noqa: E501

from __future__ import annotations

import base64
import binascii
import json
import re
from collections.abc import Mapping
from datetime import UTC, datetime, timedelta
from typing import Final

from fastapi.responses import JSONResponse, Response
from omni_connector import ActionDefinition, ActionResponse

from .client import (
    AuthenticationError,
    ForbiddenError,
    NotFoundError,
    ObjectDescribe,
    SalesforceClient,
    SalesforceClientError,
)
from .models import SalesforceAuth, SalesforceSourceConfig, _as_int, _as_str

MAX_CASE_RESULTS: Final = 50
MAX_EMAIL_RESULTS: Final = 50
MAX_FILE_RESULTS: Final = 50
MAX_ATTACHMENTS: Final = 10
MAX_FILE_BYTES: Final = 25 * 1024 * 1024
CASE_STANDARD_FIELDS: Final[tuple[str, ...]] = (
    "Id",
    "CaseNumber",
    "Subject",
    "Description",
    "Status",
    "Priority",
    "Type",
    "Origin",
    "ContactId",
    "AccountId",
    "OwnerId",
    "Owner.Name",
    "RecordTypeId",
    "RecordType.Name",
    "CreatedById",
    "CreatedDate",
    "LastModifiedDate",
    "LastActivityDate",
    "ClosedDate",
    "IsClosed",
    "Account.Name",
    "Contact.Name",
)


def _obj(properties: dict[str, object], required: list[str] | None = None) -> dict[str, object]:
    result: dict[str, object] = {"type": "object", "properties": properties}
    if required:
        result["required"] = required
    return result


def _text(description: str) -> dict[str, str]:
    return {"type": "string", "description": description}


def _definition(
    name: str,
    description: str,
    input_schema: dict[str, object],
    mode: str,
    credential_scope: str,
    source_types: list[str],
) -> ActionDefinition:
    return ActionDefinition(
        name=name,
        description=description,
        input_schema=input_schema,
        mode=mode,
        credential_scope=credential_scope,
        source_types=source_types,
    )


CASE_ACTION_DEFINITIONS: tuple[ActionDefinition, ...] = (
    _definition(
        "get_case_options",
        "Discover active Case record types and optional picklist values",
        _obj({"record_type_id": _text("Optional active Case RecordType Id")}),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "get_case_detail",
        "Read a Salesforce Case using readable standard fields",
        _obj({"case_id": _text("Salesforce Case Id")}, ["case_id"]),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "update_case",
        "Update typed standard Salesforce Case fields",
        _obj(
            {
                "case_id": _text("Salesforce Case Id"),
                "status": _text("Case status"),
                "priority": _text("Case priority"),
                "subject": _text("Case subject"),
                "description": _text("Case description"),
                "type": _text("Case type"),
                "origin": _text("Case origin"),
                "account_id": _text("Account Id"),
                "contact_id": _text("Contact Id"),
            },
            ["case_id"],
        ),
        "write",
        "org",
        ["salesforce"],
    ),
    _definition(
        "route_case",
        "Assign a Case to a Salesforce queue that supports Case",
        _obj(
            {"case_id": _text("Salesforce Case Id"), "queue_id": _text("Salesforce queue Id")},
            ["case_id", "queue_id"],
        ),
        "write",
        "org",
        ["salesforce"],
    ),
    _definition(
        "list_case_queues",
        "List Case-capable Salesforce queues and visible Case counts",
        _obj({"limit": {"type": "integer"}, "cursor": _text("Opaque QueueSobject cursor")}),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "get_case_requester_profile",
        "Read standard Contact, Account, and creator context separately",
        _obj({"case_id": _text("Salesforce Case Id")}, ["case_id"]),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "list_stale_cases",
        "List Salesforce Cases with a deterministic cursor",
        _obj(
            {
                "stale_after_seconds": {"type": "integer"},
                "limit": {"type": "integer"},
                "cursor": _text("Opaque keyset cursor"),
            }
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "list_new_cases",
        "List Salesforce Cases in an explicit UTC window",
        _obj(
            {
                "start_at": _text("UTC ISO timestamp"),
                "end_at": _text("UTC ISO timestamp"),
                "limit": {"type": "integer"},
                "cursor": _text("Opaque keyset cursor"),
            },
            ["start_at", "end_at"],
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "get_case_email_thread",
        "Read bounded Case EmailMessage rows in stable order",
        _obj(
            {
                "case_id": _text("Salesforce Case Id"),
                "limit": {"type": "integer"},
                "cursor": _text("Opaque keyset cursor"),
            },
            ["case_id"],
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "list_inbound_replies",
        "List inbound Case replies in a UTC window",
        _obj(
            {
                "start_at": _text("UTC ISO timestamp"),
                "end_at": _text("UTC ISO timestamp"),
                "limit": {"type": "integer"},
                "cursor": _text("Opaque keyset cursor"),
            },
            ["start_at", "end_at"],
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "list_case_files",
        "List files linked to a Case with an opaque cursor",
        _obj(
            {
                "case_id": _text("Salesforce Case Id"),
                "limit": {"type": "integer"},
                "cursor": _text("Opaque ContentDocument keyset cursor"),
            },
            ["case_id"],
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "fetch_case_file",
        "Download a file only when linked to the requested Case",
        _obj(
            {
                "case_id": _text("Salesforce Case Id"),
                "content_version_id": _text("ContentVersion Id"),
            },
            ["case_id", "content_version_id"],
        ),
        "read",
        "org",
        ["salesforce"],
    ),
    _definition(
        "reply_to_case",
        "Reply through Salesforce emailSimple using the latest inbound sender",
        _obj(
            {
                "case_id": _text("Salesforce Case Id"),
                "body": _text("Untrusted reply body"),
                "subject": _text("Optional subject"),
                "file_ids": {"type": "array", "items": {"type": "string"}},
            },
            ["case_id", "body"],
        ),
        "write",
        "org",
        ["salesforce"],
    ),
)


def _client(credentials: Mapping[str, object], config: SalesforceSourceConfig) -> SalesforceClient:
    return SalesforceClient(
        SalesforceAuth.from_mapping(credentials), instance_url=config.instance_url
    )


def _limit(raw: object, default: int = 20, maximum: int = MAX_CASE_RESULTS) -> int:
    value = _as_int(raw) if raw is not None else None
    if raw is not None and value is None:
        raise ValueError("limit must be an integer")
    return max(1, min(value if value is not None else default, maximum))


def _escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace("'", "\\'")


def _require_id(raw: object, prefix: str, field_name: str) -> str:
    value = _as_str(raw)
    if (
        value is None
        or re.fullmatch(rf"{re.escape(prefix)}[A-Za-z0-9]{{12}}(?:[A-Za-z0-9]{{3}})?", value)
        is None
    ):
        raise ValueError(f"{field_name} must be a valid Salesforce {prefix} id")
    return value


def _decode_cursor(
    raw: object,
    *,
    ids: bool = False,
    prefixes: tuple[str | None, str | None] | None = None,
) -> tuple[str, str] | None:
    value = _as_str(raw)
    if value is None:
        return None
    try:
        decoded = json.loads(base64.urlsafe_b64decode(value.encode("ascii")).decode("utf-8"))
    except (ValueError, UnicodeDecodeError, json.JSONDecodeError, binascii.Error) as exc:
        raise ValueError("cursor is invalid") from exc
    if (
        not isinstance(decoded, list)
        or len(decoded) != 2
        or not all(isinstance(x, str) for x in decoded)
    ):
        raise ValueError("cursor is invalid")
    if ids:
        if any(re.fullmatch(r"[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?", x) is None for x in decoded):
            raise ValueError("cursor is invalid")
    else:
        try:
            parsed = datetime.fromisoformat(decoded[0].replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                raise ValueError
        except ValueError as exc:
            raise ValueError("cursor is invalid") from exc
        decoded[0] = parsed.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
        if re.fullmatch(r"[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?", decoded[1]) is None:
            raise ValueError("cursor is invalid")
    if prefixes is not None and any(
        prefix is not None and not value.startswith(prefix)
        for value, prefix in zip(decoded, prefixes)
    ):
        raise ValueError("cursor is invalid")
    return decoded[0], decoded[1]


def _next_cursor(key: tuple[str, str] | None) -> str | None:
    if key is None:
        return None
    return base64.urlsafe_b64encode(json.dumps(list(key), separators=(",", ":")).encode()).decode()


def _response_error(exc: Exception) -> JSONResponse:
    if isinstance(exc, ValueError):
        return ActionResponse.failure(str(exc)).to_response(status_code=400)
    if isinstance(exc, NotFoundError):
        return ActionResponse.failure(str(exc)).to_response(status_code=404)
    if isinstance(exc, ForbiddenError):
        return ActionResponse.failure(str(exc)).to_response(status_code=403)
    if isinstance(exc, AuthenticationError):
        return ActionResponse.failure(str(exc)).to_response(status_code=401)
    return ActionResponse.failure(f"Salesforce API error: {exc}").to_response(status_code=502)


async def _describe_object(
    client: SalesforceClient,
    object_type: str,
    *,
    query: bool = False,
    create: bool = False,
    update: bool = False,
) -> ObjectDescribe:
    describe = await client.describe_object(object_type)
    capability = (
        ("queryable", describe.queryable, "query")
        if query
        else ("createable", describe.createable, "create")
        if create
        else ("updateable", describe.updateable, "update")
        if update
        else None
    )
    if capability is not None and not capability[1]:
        raise ForbiddenError(f"Salesforce {object_type} object is not {capability[2]}able")
    return describe


async def _field_path_readable(client: SalesforceClient, object_type: str, field: str) -> bool:
    describe = await _describe_object(client, object_type, query=True)
    if "." not in field:
        return describe.can_select(field)
    relationship, _, child = field.partition(".")
    metadata = next(
        (
            item
            for item in describe.field_metadata.values()
            if item.relationship_name == relationship
        ),
        None,
    )
    if metadata is None or not metadata.reference_to:
        return False
    related = await _describe_object(client, metadata.reference_to[0], query=True)
    return related.can_select(child)


async def _readable_fields(
    client: SalesforceClient, object_type: str, candidates: tuple[str, ...]
) -> tuple[str, ...]:
    result = [
        field
        for field in dict.fromkeys(candidates)
        if await _field_path_readable(client, object_type, field)
    ]
    if "Id" not in result:
        raise ValueError(f"Salesforce {object_type} Id field is not readable")
    return tuple(result)


async def _case_fields(client: SalesforceClient) -> tuple[str, ...]:
    return await _readable_fields(client, "Case", CASE_STANDARD_FIELDS)


async def _case(
    client: SalesforceClient, raw: object, fields: tuple[str, ...] = ()
) -> tuple[str, Mapping[str, object]]:
    case_id = _require_id(raw, "500", "case_id")
    selected = await _readable_fields(
        client, "Case", tuple(dict.fromkeys(("Id", "RecordTypeId", *fields)))
    )
    return case_id, await client.get_record("Case", case_id, selected)


def _case_json(raw: Mapping[str, object]) -> dict[str, object]:
    return {
        "id": raw.get("Id"),
        "case_number": raw.get("CaseNumber"),
        "subject": raw.get("Subject"),
        "description": raw.get("Description"),
        "status": raw.get("Status"),
        "priority": raw.get("Priority"),
        "type": raw.get("Type"),
        "origin": raw.get("Origin"),
        "contact_id": raw.get("ContactId"),
        "account_id": raw.get("AccountId"),
        "owner_id": raw.get("OwnerId"),
        "owner": raw.get("Owner"),
        "record_type_id": raw.get("RecordTypeId"),
        "record_type": raw.get("RecordType"),
        "contact": raw.get("Contact"),
        "account": raw.get("Account"),
        "created_date": raw.get("CreatedDate"),
        "last_modified_date": raw.get("LastModifiedDate"),
        "last_activity_date": raw.get("LastActivityDate"),
        "closed_date": raw.get("ClosedDate"),
        "is_closed": raw.get("IsClosed"),
    }


async def get_case_options(
    credentials: Mapping[str, object],
    params: Mapping[str, object],
    config: SalesforceSourceConfig,
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        record_types = await client.query(
            "SELECT Id, Name, DeveloperName, IsActive FROM RecordType "
            "WHERE SobjectType = 'Case' AND IsActive = true ORDER BY Name LIMIT 100"
        )
        requested = params.get("record_type_id")
        requested_id = (
            _require_id(requested, "012", "record_type_id") if requested is not None else None
        )
        options: list[dict[str, object]] = []
        for record_type in record_types.records:
            record_type_id = record_type.get("Id")
            if not isinstance(record_type_id, str):
                continue
            if requested_id is not None and record_type_id != requested_id:
                continue
            option = dict(record_type)
            if requested_id == record_type_id:
                statuses = await client.get_record_type_picklist_values(
                    "Case", record_type_id, "Status"
                )
                priorities = await client.get_record_type_picklist_values(
                    "Case", record_type_id, "Priority"
                )
                option.update(statuses=list(statuses), priorities=list(priorities))
            options.append(option)
        if requested_id is not None and not options:
            raise ValueError("record_type_id is not an active Case RecordType")
        return ActionResponse.success({"record_types": options}).to_response()
    except Exception as exc:
        return _response_error(exc)


async def get_case_detail(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        fields = await _case_fields(client)
        result = await client.query(
            f"SELECT {', '.join(fields)} FROM Case WHERE Id = '{_escape(case_id)}' LIMIT 1"
        )
        if not result.records:
            raise NotFoundError(case_id)
        return ActionResponse.success(_case_json(result.records[0])).to_response()
    except Exception as exc:
        return _response_error(exc)


async def _case_queue(client: SalesforceClient, queue_id: str) -> None:
    result = await client.query(
        f"SELECT Id FROM QueueSobject WHERE QueueId = '{_escape(queue_id)}' AND SobjectType = 'Case' LIMIT 1"
    )
    if not result.records:
        raise ValueError("queue does not support Case")


async def _case_write_fields(
    client: SalesforceClient, payload: Mapping[str, object], *, create: bool = False
) -> None:
    describe = await _describe_object(client, "Case", create=create, update=not create)
    for field, value in payload.items():
        metadata = describe.field_metadata.get(field)
        if metadata is None or not (metadata.createable if create else metadata.updateable):
            raise ValueError(f"Case field is not writable: {field}")
        if value is None and not metadata.nillable:
            raise ValueError(f"Case field cannot be cleared: {field}")


async def create_case(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        subject = _as_str(params.get("subject"))
        if subject is None:
            raise ValueError("subject is required")
        client = _client(credentials, config)
        payload: dict[str, object] = {"Subject": subject}
        fields = (
            ("description", "Description"),
            ("status", "Status"),
            ("priority", "Priority"),
            ("type", "Type"),
            ("origin", "Origin"),
        )
        for parameter, field in fields:
            if parameter in params:
                value = _as_str(params.get(parameter))
                if value is not None:
                    payload[field] = value
        for parameter, field, prefix in (
            ("account_id", "AccountId", "001"),
            ("contact_id", "ContactId", "003"),
            ("owner_id", "OwnerId", "00G"),
        ):
            if parameter in params and params.get(parameter) is not None:
                payload[field] = _require_id(params.get(parameter), prefix, parameter)
        record_type_id = params.get("record_type_id")
        if record_type_id is not None:
            record_type = _require_id(record_type_id, "012", "record_type_id")
            valid = await client.query(
                f"SELECT Id FROM RecordType WHERE Id = '{_escape(record_type)}' AND SobjectType = 'Case' AND IsActive = true LIMIT 1"
            )
            if not valid.records:
                raise ValueError("record_type_id is not an active Case RecordType")
            payload["RecordTypeId"] = record_type
            if "Status" in payload:
                statuses = await client.get_record_type_picklist_values(
                    "Case", record_type, "Status"
                )
                if payload["Status"] not in statuses:
                    raise ValueError("status is not valid for the Case record type")
        if "OwnerId" in payload:
            await _case_queue(client, str(payload["OwnerId"]))
        await _case_write_fields(client, payload, create=True)
        return ActionResponse.success(
            {"case_id": await client.create("Case", payload)}
        ).to_response(status_code=201)
    except Exception as exc:
        return _response_error(exc)


async def update_case(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, existing = await _case(client, params.get("case_id"), ("RecordTypeId",))
        payload: dict[str, object] = {}
        mapping = (
            ("status", "Status"),
            ("priority", "Priority"),
            ("subject", "Subject"),
            ("description", "Description"),
            ("type", "Type"),
            ("origin", "Origin"),
        )
        for parameter, field in mapping:
            if parameter in params:
                payload[field] = _as_str(params.get(parameter))
        for parameter, field, prefix in (
            ("account_id", "AccountId", "001"),
            ("contact_id", "ContactId", "003"),
        ):
            if parameter in params:
                payload[field] = (
                    _require_id(params.get(parameter), prefix, parameter)
                    if params.get(parameter) is not None
                    else None
                )
        record_type = existing.get("RecordTypeId")
        if "Status" in payload and isinstance(record_type, str):
            statuses = await client.get_record_type_picklist_values("Case", record_type, "Status")
            if payload["Status"] not in statuses:
                raise ValueError("status is not valid for the Case record type")
        if not payload:
            raise ValueError("at least one standard Case field is required")
        await _case_write_fields(client, payload)
        await client.update("Case", case_id, payload)
        return ActionResponse.success(
            {"case_id": case_id, "updated_fields": sorted(payload)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def route_case(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        queue_id = _require_id(params.get("queue_id"), "00G", "queue_id")
        await _case_queue(client, queue_id)
        await _case_write_fields(client, {"OwnerId": queue_id})
        await client.update("Case", case_id, {"OwnerId": queue_id})
        return ActionResponse.success({"case_id": case_id, "queue_id": queue_id}).to_response()
    except Exception as exc:
        return _response_error(exc)


async def list_case_queues(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        await _describe_object(client, "Case", query=True)
        limit = _limit(params.get("limit"), maximum=MAX_CASE_RESULTS)
        cursor = _decode_cursor(params.get("cursor"), ids=True, prefixes=("00G", None))
        where = "SobjectType = 'Case'"
        if cursor:
            where += (
                f" AND (QueueId > '{_escape(cursor[0])}' OR "
                f"(QueueId = '{_escape(cursor[0])}' AND Id > '{_escape(cursor[1])}'))"
            )
        supported = await client.query(
            f"SELECT Id, QueueId FROM QueueSobject WHERE {where} "
            f"ORDER BY QueueId ASC, Id ASC LIMIT {limit + 1}"
        )
        page = list(supported.records[:limit])
        queue_ids = [row["QueueId"] for row in page if isinstance(row.get("QueueId"), str)]
        if not queue_ids:
            return ActionResponse.success({"queues": [], "next_cursor": None}).to_response()
        quoted = ", ".join(f"'{_escape(queue_id)}'" for queue_id in queue_ids)
        groups = await client.query(
            f"SELECT Id, Name, Type FROM Group WHERE Type = 'Queue' "
            f"AND Id IN ({quoted}) LIMIT {limit}"
        )
        group_by_id = {row["Id"]: row for row in groups.records if isinstance(row.get("Id"), str)}
        status_rows = await client.query(
            f"SELECT OwnerId, Status, COUNT(Id) case_count FROM Case "
            f"WHERE OwnerId IN ({quoted}) GROUP BY OwnerId, Status"
        )
        closed_rows = await client.query(
            f"SELECT OwnerId, IsClosed, COUNT(Id) case_count FROM Case "
            f"WHERE OwnerId IN ({quoted}) GROUP BY OwnerId, IsClosed"
        )
        counts: dict[str, dict[str, int]] = {queue_id: {} for queue_id in queue_ids}
        for row in status_rows.records:
            if (
                isinstance(row.get("OwnerId"), str)
                and isinstance(row.get("Status"), str)
                and isinstance(row.get("case_count"), (int, float))
            ):
                counts[row["OwnerId"]][row["Status"]] = int(row["case_count"])
        open_counts = {queue_id: 0 for queue_id in queue_ids}
        for row in closed_rows.records:
            if (
                isinstance(row.get("OwnerId"), str)
                and row.get("IsClosed") is not True
                and isinstance(row.get("case_count"), (int, float))
            ):
                open_counts[row["OwnerId"]] = int(row["case_count"])
        result: list[dict[str, object]] = []
        for queue_id in queue_ids:
            group = group_by_id.get(queue_id)
            if group is None:
                continue
            result.append(
                {
                    **dict(group),
                    "total_count": sum(counts[queue_id].values()),
                    "open_count": open_counts[queue_id],
                    "status_counts": counts[queue_id],
                }
            )
        next_key = None
        if len(supported.records) > limit and page:
            last = page[-1]
            if isinstance(last.get("QueueId"), str) and isinstance(last.get("Id"), str):
                next_key = (last["QueueId"], last["Id"])
        return ActionResponse.success(
            {"queues": result, "next_cursor": _next_cursor(next_key)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def _list_cases(
    credentials: Mapping[str, object],
    params: Mapping[str, object],
    config: SalesforceSourceConfig,
    *,
    new: bool,
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        limit = _limit(params.get("limit"))
        cursor = _decode_cursor(params.get("cursor"), prefixes=(None, "500"))
        if new:
            start = _parse_datetime(params.get("start_at"), "start_at")
            end = _parse_datetime(params.get("end_at"), "end_at")
            if end < start:
                raise ValueError("end_at must not precede start_at")
            time_field = "CreatedDate"
            where = f"CreatedDate >= {start.strftime('%Y-%m-%dT%H:%M:%SZ')} AND CreatedDate <= {end.strftime('%Y-%m-%dT%H:%M:%SZ')}"
        else:
            raw_seconds = params.get("stale_after_seconds")
            seconds = _as_int(raw_seconds) if raw_seconds is not None else 86400
            if seconds is None or seconds < 0:
                raise ValueError("stale_after_seconds must be a non-negative integer")
            time_field = "LastModifiedDate"
            cutoff = datetime.now(UTC) - timedelta(seconds=seconds)
            where = (
                f"IsClosed = false AND LastModifiedDate < {cutoff.strftime('%Y-%m-%dT%H:%M:%SZ')}"
            )
        if cursor:
            where += f" AND ({time_field} > {cursor[0]} OR ({time_field} = {cursor[0]} AND Id > '{_escape(cursor[1])}'))"
        fields = await _case_fields(client)
        result = await client.query(
            f"SELECT {', '.join(fields)} FROM Case WHERE {where} ORDER BY {time_field} ASC, Id ASC LIMIT {limit + 1}"
        )
        page = list(result.records[:limit])
        key = None
        if (
            len(result.records) > limit
            and page
            and isinstance(page[-1].get(time_field), str)
            and isinstance(page[-1].get("Id"), str)
        ):
            key = (page[-1][time_field], page[-1]["Id"])
        return ActionResponse.success(
            {"cases": [_case_json(row) for row in page], "next_cursor": _next_cursor(key)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


def _parse_datetime(value: object, name: str) -> datetime:
    parsed = _as_str(value)
    if parsed is None:
        raise ValueError(f"{name} is required")
    try:
        result = datetime.fromisoformat(parsed.replace("Z", "+00:00"))
    except ValueError as exc:
        raise ValueError(f"{name} must be an ISO timestamp") from exc
    if result.tzinfo is None:
        raise ValueError(f"{name} must include a timezone")
    return result.astimezone(UTC)


async def list_stale_cases(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    return await _list_cases(credentials, params, config, new=False)


async def list_new_cases(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    return await _list_cases(credentials, params, config, new=True)


async def get_case_requester_profile(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, case = await _case(
            client, params.get("case_id"), ("ContactId", "AccountId", "CreatedById")
        )
        contact_fields = await _readable_fields(
            client, "Contact", ("Id", "Name", "Email", "Phone", "AccountId")
        )
        account_fields = await _readable_fields(
            client,
            "Account",
            (
                "Id",
                "Name",
                "Industry",
                "Phone",
                "Website",
                "BillingCity",
                "BillingState",
                "BillingCountry",
                "Type",
            ),
        )
        creator_fields = await _readable_fields(
            client, "User", ("Id", "Name", "Email", "Title", "Department", "IsActive")
        )
        contact = (
            await client.get_record("Contact", case["ContactId"], contact_fields)
            if isinstance(case.get("ContactId"), str)
            else None
        )
        account = (
            await client.get_record("Account", case["AccountId"], account_fields)
            if isinstance(case.get("AccountId"), str)
            else None
        )
        creator = (
            await client.get_record("User", case["CreatedById"], creator_fields)
            if isinstance(case.get("CreatedById"), str)
            else None
        )
        return ActionResponse.success(
            {"contact": contact, "account": account, "creator": creator}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def _email_rows(
    client: SalesforceClient,
    case_id: str,
    limit: int,
    cursor: tuple[str, str] | None = None,
    incoming: bool | None = None,
    window: tuple[datetime, datetime] | None = None,
    descending: bool = False,
) -> tuple[list[Mapping[str, object]], tuple[str, str] | None]:
    where = f"ParentId = '{_escape(case_id)}'"
    if incoming is not None:
        where += f" AND Incoming = {'true' if incoming else 'false'}"
    if window:
        where += f" AND MessageDate >= {window[0].strftime('%Y-%m-%dT%H:%M:%SZ')} AND MessageDate <= {window[1].strftime('%Y-%m-%dT%H:%M:%SZ')}"
    if cursor:
        where += f" AND (MessageDate > {cursor[0]} OR (MessageDate = {cursor[0]} AND Id > '{_escape(cursor[1])}'))"
    order = "DESC" if descending else "ASC"
    result = await client.query(
        f"SELECT Id, ParentId, FromAddress, ToAddress, CcAddress, Subject, TextBody, HtmlBody, Incoming, MessageDate, CreatedDate FROM EmailMessage WHERE {where} ORDER BY MessageDate {order}, Id {order} LIMIT {limit + 1}"
    )
    rows = list(result.records[:limit])
    key = None
    if (
        len(result.records) > limit
        and rows
        and isinstance(rows[-1].get("MessageDate"), str)
        and isinstance(rows[-1].get("Id"), str)
    ):
        key = (rows[-1]["MessageDate"], rows[-1]["Id"])
    return rows, key


def _email_json(row: Mapping[str, object]) -> dict[str, object]:
    incoming = row.get("Incoming") is True
    return {
        "id": row.get("Id"),
        "from": row.get("FromAddress"),
        "to": row.get("ToAddress"),
        "cc": row.get("CcAddress"),
        "subject": row.get("Subject"),
        "text_body": row.get("TextBody"),
        "html_body": row.get("HtmlBody"),
        "incoming": incoming,
        "direction": "inbound" if incoming else "outbound",
        "message_date": row.get("MessageDate"),
        "created_date": row.get("CreatedDate"),
    }


async def get_case_email_thread(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        rows, key = await _email_rows(
            client,
            case_id,
            _limit(params.get("limit"), maximum=MAX_EMAIL_RESULTS),
            _decode_cursor(params.get("cursor"), prefixes=(None, "02s")),
        )
        return ActionResponse.success(
            {"emails": [_email_json(row) for row in rows], "next_cursor": _next_cursor(key)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def list_inbound_replies(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        start = _parse_datetime(params.get("start_at"), "start_at")
        end = _parse_datetime(params.get("end_at"), "end_at")
        if end < start:
            raise ValueError("end_at must not precede start_at")
        client = _client(credentials, config)
        limit = _limit(params.get("limit"), maximum=MAX_EMAIL_RESULTS)
        where = f"Incoming = true AND MessageDate >= {start.strftime('%Y-%m-%dT%H:%M:%SZ')} AND MessageDate <= {end.strftime('%Y-%m-%dT%H:%M:%SZ')} AND ParentId IN (SELECT Id FROM Case WHERE Id != null)"
        cursor = _decode_cursor(params.get("cursor"), prefixes=(None, "02s"))
        if cursor:
            where += f" AND (MessageDate > {cursor[0]} OR (MessageDate = {cursor[0]} AND Id > '{_escape(cursor[1])}'))"
        result = await client.query(
            f"SELECT Id, ParentId, FromAddress, ToAddress, CcAddress, Subject, TextBody, HtmlBody, Incoming, MessageDate, CreatedDate FROM EmailMessage WHERE {where} ORDER BY MessageDate ASC, Id ASC LIMIT {limit + 1}"
        )
        page = list(result.records[:limit])
        key = (
            (page[-1]["MessageDate"], page[-1]["Id"])
            if len(result.records) > limit and page
            else None
        )
        return ActionResponse.success(
            {"emails": [_email_json(row) for row in page], "next_cursor": _next_cursor(key)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def list_case_files(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        limit = _limit(params.get("limit"), maximum=MAX_FILE_RESULTS)
        cursor = _decode_cursor(params.get("cursor"), ids=True, prefixes=("069", "06A"))
        where = f"LinkedEntityId = '{_escape(case_id)}'"
        if cursor:
            where += f" AND (ContentDocumentId > '{_escape(cursor[0])}' OR (ContentDocumentId = '{_escape(cursor[0])}' AND Id > '{_escape(cursor[1])}'))"
        links = await client.query(
            f"SELECT Id, ContentDocumentId FROM ContentDocumentLink WHERE {where} ORDER BY ContentDocumentId ASC, Id ASC LIMIT {limit + 1}"
        )
        page_links = list(links.records[:limit])
        document_ids = [
            link["ContentDocumentId"]
            for link in page_links
            if isinstance(link.get("ContentDocumentId"), str)
        ]
        files: list[dict[str, object]] = []
        if document_ids:
            quoted = ", ".join(f"'{_escape(item)}'" for item in document_ids)
            versions = await client.query(
                f"SELECT Id, ContentDocumentId, Title, FileExtension, ContentSize, CreatedDate, IsLatest FROM ContentVersion WHERE ContentDocumentId IN ({quoted}) AND IsLatest = true ORDER BY ContentDocumentId ASC, Id ASC LIMIT {limit}"
            )
            files = [dict(row) for row in versions.records]
        key = None
        if (
            len(links.records) > limit
            and page_links
            and isinstance(page_links[-1].get("ContentDocumentId"), str)
            and isinstance(page_links[-1].get("Id"), str)
        ):
            key = (page_links[-1]["ContentDocumentId"], page_links[-1]["Id"])
        return ActionResponse.success(
            {"files": files, "next_cursor": _next_cursor(key)}
        ).to_response()
    except Exception as exc:
        return _response_error(exc)


async def fetch_case_file(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> Response | JSONResponse:
    try:
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        version_id = _require_id(params.get("content_version_id"), "068", "content_version_id")
        version = await client.get_record(
            "ContentVersion",
            version_id,
            ("Id", "ContentDocumentId", "Title", "FileExtension", "ContentSize"),
        )
        document_id = version.get("ContentDocumentId")
        if not isinstance(document_id, str):
            raise SalesforceClientError("ContentVersion response missing ContentDocumentId")
        linked = await client.query(
            f"SELECT Id FROM ContentDocumentLink WHERE LinkedEntityId = '{_escape(case_id)}' AND ContentDocumentId = '{_escape(document_id)}' LIMIT 1"
        )
        if not linked.records:
            raise ForbiddenError("file is not linked to this Case")
        size = version.get("ContentSize")
        if isinstance(size, (int, float)) and size > MAX_FILE_BYTES:
            raise ValueError("file exceeds the configured download limit")
        body, content_type, response_size = await client.fetch_binary(
            f"/sobjects/ContentVersion/{version_id}/VersionData", max_bytes=MAX_FILE_BYTES
        )
        if response_size is not None and response_size > MAX_FILE_BYTES:
            raise ValueError("file exceeds the configured download limit")
        title = version.get("Title") if isinstance(version.get("Title"), str) else version_id
        filename = re.sub(r"[^A-Za-z0-9._ -]+", "_", title).strip(" .") or version_id
        extension = version.get("FileExtension")
        if (
            isinstance(extension, str)
            and extension
            and not filename.lower().endswith(f".{extension.lower()}")
        ):
            filename += f".{re.sub(r'[^A-Za-z0-9]+', '', extension)}"
        return Response(
            content=body,
            media_type=content_type or "application/octet-stream",
            headers={
                "Content-Disposition": f'attachment; filename="{filename}"',
                "X-File-Name": filename,
            },
        )
    except Exception as exc:
        return _response_error(exc)


async def reply_to_case(
    credentials: Mapping[str, object], params: Mapping[str, object], config: SalesforceSourceConfig
) -> JSONResponse:
    try:
        body = _as_str(params.get("body"))
        if body is None:
            raise ValueError("body is required")
        client = _client(credentials, config)
        case_id, _ = await _case(client, params.get("case_id"))
        latest, _ = await _email_rows(client, case_id, 1, incoming=True, descending=True)
        recipient = latest[0].get("FromAddress") if latest else None
        if (
            not isinstance(recipient, str)
            or re.fullmatch(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+", recipient) is None
        ):
            raise ValueError("could not resolve a safe Case recipient")
        file_ids = params.get("file_ids")
        if file_ids is not None and (
            not isinstance(file_ids, list)
            or len(file_ids) > MAX_ATTACHMENTS
            or not all(isinstance(item, str) for item in file_ids)
        ):
            raise ValueError(f"file_ids must contain at most {MAX_ATTACHMENTS} ContentVersion ids")
        if isinstance(file_ids, list) and file_ids:
            file_ids = [_require_id(item, "068", "file_id") for item in file_ids]
            for file_id in file_ids:
                version = await client.get_record("ContentVersion", file_id, ("ContentDocumentId",))
                document_id = version.get("ContentDocumentId")
                if not isinstance(document_id, str):
                    raise SalesforceClientError("ContentVersion response missing ContentDocumentId")
                linked = await client.query(
                    f"SELECT Id FROM ContentDocumentLink WHERE LinkedEntityId = '{_escape(case_id)}' AND ContentDocumentId = '{_escape(document_id)}' LIMIT 1"
                )
                if not linked.records:
                    raise ForbiddenError("attachment is not linked to this Case")
        input_row: dict[str, object] = {
            "relatedRecordId": case_id,
            "recipientAddresses": [recipient],
            "emailSubject": _as_str(params.get("subject")) or "Re: Case",
            "emailBody": body,
            "senderType": "CurrentUser",
            "logEmailOnSend": True,
            "addThreadingTokenToSubject": True,
            "addThreadingTokenToBody": True,
        }
        if isinstance(file_ids, list) and file_ids:
            input_row["attachmentIdCollection"] = file_ids
        results = await client.invoke_standard_action("/emailSimple", {"inputs": [input_row]})
        failures = [
            error.describe()
            for result in results
            if not result.is_success
            for error in result.errors
        ]
        if any(not result.is_success for result in results):
            raise SalesforceClientError(
                "Salesforce emailSimple failed: " + ("; ".join(failures) or "no error details")
            )
        return ActionResponse.success({"case_id": case_id, "recipient": recipient}).to_response()
    except Exception as exc:
        return _response_error(exc)


async def execute_case_action(
    action: str,
    params: Mapping[str, object],
    credentials: Mapping[str, object],
    config: SalesforceSourceConfig,
) -> JSONResponse | Response:
    handlers = {
        "create_case": lambda: create_case(credentials, params, config),
        "get_case_options": lambda: get_case_options(credentials, params, config),
        "get_case_detail": lambda: get_case_detail(credentials, params, config),
        "update_case": lambda: update_case(credentials, params, config),
        "route_case": lambda: route_case(credentials, params, config),
        "list_case_queues": lambda: list_case_queues(credentials, params, config),
        "get_case_requester_profile": lambda: get_case_requester_profile(
            credentials, params, config
        ),
        "list_stale_cases": lambda: list_stale_cases(credentials, params, config),
        "list_new_cases": lambda: list_new_cases(credentials, params, config),
        "get_case_email_thread": lambda: get_case_email_thread(credentials, params, config),
        "list_inbound_replies": lambda: list_inbound_replies(credentials, params, config),
        "list_case_files": lambda: list_case_files(credentials, params, config),
        "fetch_case_file": lambda: fetch_case_file(credentials, params, config),
        "reply_to_case": lambda: reply_to_case(credentials, params, config),
    }
    handler = handlers.get(action)
    if handler is None:
        return ActionResponse.not_supported(action).to_response(status_code=404)
    return await handler()
