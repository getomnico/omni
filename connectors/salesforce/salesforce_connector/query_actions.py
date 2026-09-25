"""User-scoped, read-only Salesforce query actions replacing the DX MCP subset."""

from __future__ import annotations

import json
from collections.abc import Mapping
from urllib.parse import urlencode

from fastapi.responses import JSONResponse, Response
from omni_connector import ActionDefinition, ActionResponse

from .client import (
    AuthenticationError,
    ForbiddenError,
    SalesforceClient,
    SalesforceClientError,
    _verified_salesforce_host,
    _verified_salesforce_login_url,
    fetch_user_identity,
)
from .models import SalesforceAuth, SalesforceSourceConfig

MAX_QUERY_BYTES = 16_384
MAX_QUERY_PAGES = 100
MAX_QUERY_RECORDS = 10_000
MAX_RESULT_BYTES = 8_000_000
MAX_QUERY_SECONDS = 90

_QUERY_SCHEMA: dict[str, object] = {
    "type": "object",
    "properties": {
        "query": {"type": "string", "description": "Read-only SOQL SELECT query"},
        "usernameOrAlias": {
            "type": "string",
            "description": "The authorized Salesforce user's username (from get_username)",
        },
        "directory": {
            "type": "string",
            "description": "Retained for contract compatibility; no filesystem access is performed",
        },
        "useToolingApi": {
            "type": "boolean",
            "description": "Query through the Salesforce Tooling API",
        },
    },
    "required": ["query", "usernameOrAlias", "directory"],
}
_USERNAME_SCHEMA: dict[str, object] = {
    "type": "object",
    "properties": {
        "directory": {
            "type": "string",
            "description": "Retained for contract compatibility; no filesystem access is performed",
        },
        "defaultTargetOrg": {"type": "boolean", "default": False},
        "defaultDevHub": {"type": "boolean", "default": False},
    },
    "required": ["directory"],
}

QUERY_ACTION_DEFINITIONS = (
    ActionDefinition(
        name="run_soql_query",
        description="Run a bounded read-only SOQL query as the acting Salesforce user.",
        input_schema=_QUERY_SCHEMA,
        mode="read",
        credential_scope="user",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="get_username",
        description="Resolve the acting user's verified Salesforce username for this source org.",
        input_schema=_USERNAME_SCHEMA,
        mode="read",
        credential_scope="user",
        source_types=["salesforce"],
    ),
)


def _validate_read_query(query: object) -> str:
    if not isinstance(query, str) or not query.strip():
        raise ValueError("query is required")
    if len(query.encode("utf-8")) > MAX_QUERY_BYTES:
        raise ValueError("query exceeds the maximum length")
    # Scan tokens outside SOQL string literals; reject comments, statement
    # separators, and mutating/locking clauses without matching quoted text.
    tokens: list[str] = []
    index = 0
    while index < len(query):
        char = query[index]
        if char == "'":
            index += 1
            while index < len(query):
                if query[index] == "\\":
                    index += 2
                elif query[index] == "'":
                    if index + 1 < len(query) and query[index + 1] == "'":
                        index += 2
                    else:
                        index += 1
                        break
                else:
                    index += 1
            else:
                raise ValueError("query contains an unterminated string literal")
            continue
        if char == ";" or query.startswith(("--", "/*", "//"), index):
            raise ValueError("query must be one comment-free SELECT statement")
        if char.isalpha() or char == "_":
            end = index + 1
            while end < len(query) and (query[end].isalnum() or query[end] == "_"):
                end += 1
            tokens.append(query[index:end].upper())
            index = end
        else:
            index += 1
    if not tokens or tokens[0] != "SELECT":
        raise ValueError("only SOQL SELECT queries are allowed")
    forbidden = {
        "INSERT",
        "UPDATE",
        "UPSERT",
        "DELETE",
        "UNDELETE",
        "MERGE",
        "DROP",
        "CREATE",
        "ALTER",
        "FOR",
    }
    if forbidden.intersection(tokens):
        raise ValueError("query contains a disallowed mutating or locking clause")
    return query.strip()


def _required_text(params: Mapping[str, object], key: str) -> str:
    value = params.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{key} is required")
    return value.strip()


def _source_org(
    source_config: SalesforceSourceConfig, source: Mapping[str, object] | None
) -> tuple[str | None, str | None]:
    config = source or {}
    binding = config.get("source_binding")
    binding_map = binding if isinstance(binding, Mapping) else {}
    organization_id = binding_map.get("organization_id", config.get("organization_id"))
    instance_url = binding_map.get("instance_url", config.get("instance_url"))
    expected_org = organization_id if isinstance(organization_id, str) and organization_id else None
    expected_instance = (
        instance_url
        if isinstance(instance_url, str) and instance_url
        else source_config.instance_url
    )
    return expected_org, expected_instance


async def _verified_client(
    params: Mapping[str, object],
    credentials: Mapping[str, object],
    source_config: SalesforceSourceConfig,
    source: Mapping[str, object] | None,
) -> tuple[SalesforceClient, str]:
    actor_username_value = params.get("usernameOrAlias")
    if actor_username_value is not None and not isinstance(actor_username_value, str):
        raise ValueError("usernameOrAlias must be a string")
    if isinstance(actor_username_value, str) and not actor_username_value.strip():
        raise ValueError("usernameOrAlias cannot be empty")
    actor_username = (
        actor_username_value.strip()
        if isinstance(actor_username_value, str) and actor_username_value.strip()
        else None
    )
    auth_credentials = dict(credentials)
    source_login_value = source.get("login_url") if source is not None else None
    credential_login_value = auth_credentials.get("login_url")
    if source_login_value is not None and not isinstance(source_login_value, str):
        raise ValueError("Salesforce source login URL is malformed")
    if credential_login_value is not None and not isinstance(credential_login_value, str):
        raise ValueError("Salesforce credential login URL is malformed")
    source_login_url = (
        _validated_login_url(source_login_value) if isinstance(source_login_value, str) else None
    )
    credential_login_url = (
        _validated_login_url(credential_login_value)
        if isinstance(credential_login_value, str)
        else None
    )
    if (
        isinstance(credential_login_url, str)
        and isinstance(source_login_url, str)
        and credential_login_url != source_login_url
    ):
        raise ForbiddenError("Salesforce login host does not match this source")
    selected_login_url = source_login_url or credential_login_url or "https://login.salesforce.com"
    auth_credentials["login_url"] = _validated_login_url(selected_login_url)
    auth = SalesforceAuth.from_mapping(auth_credentials)
    if auth.mode.value != "bearer" or not auth.access_token:
        raise ValueError("A user OAuth credential is required")
    if not auth.instance_url:
        raise ValueError("Salesforce user credential has no instance URL")
    asserted_instance = _canonical_host(auth.instance_url)
    expected_org, expected_instance = _source_org(source_config, source)
    if not expected_org:
        raise ForbiddenError("Salesforce source has no verified organization binding")
    if expected_instance and _canonical_host(expected_instance) != asserted_instance:
        raise ForbiddenError("Salesforce credential instance does not match this source")
    identity = await fetch_user_identity(auth)
    if identity.organization_id != expected_org:
        raise ForbiddenError("Salesforce credential does not belong to this source organization")
    verified_instance = _canonical_host(identity.instance_url)
    if verified_instance != asserted_instance:
        raise ForbiddenError("Salesforce instance URL does not match provider identity")
    if expected_instance and verified_instance != _canonical_host(expected_instance):
        raise ForbiddenError("Salesforce credential instance does not match this source")
    if actor_username is not None and actor_username.casefold() not in {
        identity.username.casefold(),
        identity.user_id.casefold(),
    }:
        raise ValueError("usernameOrAlias does not identify the acting Salesforce user")
    return SalesforceClient(auth, instance_url=identity.instance_url), identity.username


def _validated_login_url(value: str) -> str:
    try:
        return _verified_salesforce_login_url(value)
    except (SalesforceClientError, ValueError) as exc:
        raise ValueError("Invalid Salesforce login URL") from exc


def _canonical_host(value: str) -> str:
    try:
        return _verified_salesforce_host(value)
    except (SalesforceClientError, ValueError) as exc:
        raise ValueError("Invalid Salesforce instance URL") from exc


async def _all_query_pages(client: SalesforceClient, query: str, tooling: bool) -> str:
    from asyncio import timeout

    records: list[Mapping[str, object]] = []
    encoded_record_bytes = 0
    async with timeout(MAX_QUERY_SECONDS):
        result = await (client.query_tooling(query) if tooling else client.query(query))
        if result.total_size > MAX_QUERY_RECORDS:
            raise SalesforceClientError("Salesforce query result is too large; narrow the query")
        pages = 0
        while True:
            pages += 1
            if pages > MAX_QUERY_PAGES:
                raise SalesforceClientError("Salesforce query exceeded the maximum page count")
            for record in result.records:
                encoded_record_bytes += len(
                    json.dumps(record, indent=2, ensure_ascii=False).encode("utf-8")
                )
                if encoded_record_bytes > MAX_RESULT_BYTES:
                    raise SalesforceClientError(
                        "Salesforce query result exceeds the response size limit; narrow the query"
                    )
                records.append(record)
                if len(records) > MAX_QUERY_RECORDS:
                    raise SalesforceClientError(
                        "Salesforce query exceeded the maximum result count"
                    )
            if result.done:
                break
            if result.next_records_url is None:
                raise SalesforceClientError("Salesforce query pagination was incomplete")
            result = await (
                client.query_more_tooling(result.next_records_url)
                if tooling
                else client.query_more(result.next_records_url)
            )
    if result.total_size > MAX_QUERY_RECORDS:
        raise SalesforceClientError("Salesforce query result is too large; narrow the query")
    envelope = {"totalSize": result.total_size, "done": result.done, "records": records}
    rendered = "SOQL query results:\n\n" + json.dumps(envelope, indent=2, ensure_ascii=False)
    if len(rendered.encode("utf-8")) > MAX_RESULT_BYTES:
        raise SalesforceClientError(
            "Salesforce query result exceeds the response size limit; narrow the query"
        )
    return rendered


async def execute_query_action(
    action: str,
    params: Mapping[str, object],
    credentials: Mapping[str, object],
    source_config: SalesforceSourceConfig,
    source: Mapping[str, object] | None = None,
) -> JSONResponse | Response:
    try:
        _required_text(params, "directory")
        if action == "run_soql_query":
            _required_text(params, "usernameOrAlias")
        client, username = await _verified_client(params, credentials, source_config, source)
        if action == "get_username":
            default_requested = (
                params.get("defaultTargetOrg") is True or params.get("defaultDevHub") is True
            )
            prefix = (
                "Local Salesforce CLI default-org settings are unavailable to the native API. "
                if default_requested
                else ""
            )
            return ActionResponse.success(
                {
                    "content": (
                        f'{prefix}Verified Salesforce username for this source: "{username}". '
                        'Unless the user specifies otherwise, use this as "usernameOrAlias".'
                    )
                }
            ).to_response()
        query = _validate_read_query(params.get("query"))
        tooling = params.get("useToolingApi", False)
        if not isinstance(tooling, bool):
            raise ValueError("useToolingApi must be a boolean")
        text = await _all_query_pages(client, query, tooling)
        return ActionResponse.success({"content": text}).to_response()
    except ValueError as exc:
        return ActionResponse.failure(str(exc)).to_response(status_code=400)
    except AuthenticationError:
        source_id = source.get("id") if source is not None else None
        if not isinstance(source_id, str) or not source_id:
            return ActionResponse.failure(
                "Salesforce source context is required to reconnect this user credential"
            ).to_response(status_code=403)
        return JSONResponse(
            {
                "error": "needs_user_auth",
                "source_id": source_id,
                "source_type": "salesforce",
                "provider": "salesforce",
                "oauth_start_url": f"/api/oauth/start?{urlencode({'source_id': source_id})}",
            },
            status_code=412,
        )
    except ForbiddenError:
        return ActionResponse.failure(
            "Salesforce credential is not authorized for this source or lacks query permission"
        ).to_response(status_code=403)
    except SalesforceClientError:
        return ActionResponse.failure("Salesforce query failed").to_response(status_code=502)
    except Exception:
        return ActionResponse.failure("Salesforce query failed").to_response(status_code=502)
