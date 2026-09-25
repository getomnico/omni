"""Salesforce REST API client with typed responses and retry logic."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from functools import wraps
from typing import TypeVar, cast
from urllib.parse import parse_qsl, urlparse

import jwt
import requests
from simple_salesforce import (  # type: ignore[attr-defined]
    Salesforce,
    SalesforceAuthenticationFailed,
    SalesforceError,
)

from .config import API_VERSION
from .models import AuthMode, SalesforceAuth

logger = logging.getLogger(__name__)

T = TypeVar("T")

# Salesforce access tokens live ~2h; refresh this early to never hit expiry.
TOKEN_REFRESH_EARLY_SECONDS = 300

# Bounded retry for rate limiting. An unbounded 429 loop would keep the sync
# from returning control for checkpoint heartbeats and could outlive the
# manager's stale-sync timeout.
RATE_LIMIT_MAX_RETRIES = 5
RATE_LIMIT_BASE_DELAY_SECONDS = 10.0
MAX_BINARY_BYTES = 25 * 1024 * 1024
SALESFORCE_HTTP_TIMEOUT_SECONDS = 15


class SalesforceClientError(Exception):
    """Base exception for Salesforce API errors."""

    pass


class AuthenticationError(SalesforceClientError):
    """Invalid or expired token (401)."""

    pass


class ForbiddenError(SalesforceClientError):
    """Insufficient permissions (403)."""

    pass


class NotFoundError(SalesforceClientError):
    """Record not found (404)."""

    pass


def with_retry(
    max_retries: int = 3,
    base_delay: float = 1.0,
) -> Callable[[Callable[..., Awaitable[T]]], Callable[..., Awaitable[T]]]:
    """Retry Salesforce API calls with exponential backoff.

    - 401: re-raised as AuthenticationError (non-retryable)
    - 403: re-raised as ForbiddenError (non-retryable)
    - 404: re-raised as NotFoundError (non-retryable)
    - 429: exponential backoff, bounded by RATE_LIMIT_MAX_RETRIES
    - 5xx: exponential backoff, bounded by max_retries
    """

    def decorator(func: Callable[..., Awaitable[T]]) -> Callable[..., Awaitable[T]]:
        @wraps(func)
        async def wrapper(*args: object, **kwargs: object) -> T:
            last_exception: SalesforceError | None = None
            error_retries = 0
            rate_limit_retries = 0
            refreshed = False

            async def refresh_once() -> bool:
                """Refresh the session token once (JWT mode only); else False."""
                nonlocal refreshed
                if refreshed:
                    return False
                # The decorated method is called as instance.method(...), so the
                # bound instance is the first positional argument.
                bound = args[0] if args else None
                handler = getattr(bound, "_refresh_expired_token", None)
                if handler is None:
                    return False
                did_refresh = await handler()
                if not did_refresh:
                    return False
                refreshed = True
                return True

            while True:
                try:
                    return await func(*args, **kwargs)
                except SalesforceAuthenticationFailed as e:
                    if await refresh_once():
                        continue
                    raise AuthenticationError("Invalid or expired access token") from e
                except SalesforceError as e:
                    last_exception = e
                    status = getattr(e, "status", 0)

                    if status == 401:
                        if await refresh_once():
                            continue
                        raise AuthenticationError("Invalid or expired access token") from e
                    if status == 403:
                        raise ForbiddenError(f"Insufficient permissions: {e}") from e
                    if status == 404:
                        raise NotFoundError(str(e)) from e
                    if status == 429:
                        rate_limit_retries += 1
                        if rate_limit_retries > RATE_LIMIT_MAX_RETRIES:
                            raise SalesforceClientError(
                                "Rate limited by Salesforce; max retries exceeded"
                            ) from e
                        delay = RATE_LIMIT_BASE_DELAY_SECONDS * (
                            2 ** (rate_limit_retries - 1)
                        )
                        logger.warning(
                            "Rate limited. Waiting %.1fs (attempt %d/%d)",
                            delay,
                            rate_limit_retries,
                            RATE_LIMIT_MAX_RETRIES,
                        )
                        await asyncio.sleep(delay)
                        continue
                    if status >= 500:
                        error_retries += 1
                        if error_retries > max_retries:
                            break
                        delay = base_delay * (2 ** (error_retries - 1))
                        logger.warning(
                            "Server error %d. Retrying in %.1fs (attempt %d/%d)",
                            status,
                            delay,
                            error_retries,
                            max_retries,
                        )
                        await asyncio.sleep(delay)
                        continue

                    raise SalesforceClientError(f"API error: {e}") from e

            raise SalesforceClientError(
                f"Max retries exceeded: {last_exception}"
            ) from last_exception

        return wrapper

    return decorator


def _as_str(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise SalesforceClientError(
            f"malformed API response: {field} expected string, got {type(value).__name__}"
        )
    return value


def _as_int(value: object, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise SalesforceClientError(
            f"malformed API response: {field} expected number, got {type(value).__name__}"
        )
    if isinstance(value, float) and not value.is_integer():
        raise SalesforceClientError(
            f"malformed API response: {field} expected an integer, got {value!r}"
        )
    return int(value)


def _as_bool(value: object, field: str) -> bool:
    if not isinstance(value, bool):
        raise SalesforceClientError(
            f"malformed API response: {field} expected boolean, got {type(value).__name__}"
        )
    return value


@dataclass(frozen=True)
class QueryResult:
    """Typed envelope of a SOQL query response."""

    done: bool
    total_size: int
    records: tuple[Mapping[str, object], ...]
    next_records_url: str | None

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> QueryResult:
        records_value = raw.get("records")
        if not isinstance(records_value, list):
            raise SalesforceClientError(
                "malformed query response: records expected list, "
                f"got {type(records_value).__name__}"
            )
        records: list[Mapping[str, object]] = []
        for record in records_value:
            if not isinstance(record, Mapping):
                raise SalesforceClientError(
                    f"malformed query response: record expected object, got {type(record).__name__}"
                )
            records.append(record)
        next_url_value = raw.get("nextRecordsUrl")
        next_url = _as_str(next_url_value, "nextRecordsUrl") if next_url_value is not None else None
        return cls(
            done=_as_bool(raw.get("done"), "done"),
            total_size=_as_int(raw.get("totalSize"), "totalSize"),
            records=tuple(records),
            next_records_url=next_url,
        )


@dataclass(frozen=True)
class StandardActionError:
    """Typed error returned by a Salesforce invocable standard action."""

    status_code: str
    message: str
    fields: tuple[str, ...]

    def describe(self) -> str:
        suffix = f" ({', '.join(self.fields)})" if self.fields else ""
        return f"{self.status_code}: {self.message}{suffix}"


@dataclass(frozen=True)
class StandardActionResult:
    """One result from a Salesforce invocable standard action."""

    is_success: bool
    action_name: str
    errors: tuple[StandardActionError, ...]
    output_values: Mapping[str, object] | None

    @classmethod
    def from_response(cls, raw: object) -> tuple[StandardActionResult, ...]:
        if not isinstance(raw, Sequence) or isinstance(raw, (str, bytes, bytearray)):
            raise SalesforceClientError(
                "malformed standard action response: expected an array of results"
            )
        results: list[StandardActionResult] = []
        for item in raw:
            if not isinstance(item, Mapping) or not isinstance(item.get("isSuccess"), bool):
                raise SalesforceClientError("malformed standard action result")
            action_name = item.get("actionName")
            if not isinstance(action_name, str) or not action_name:
                raise SalesforceClientError("malformed standard action actionName")
            if "errors" not in item or "outputValues" not in item:
                raise SalesforceClientError("malformed standard action result fields")
            errors_value = item["errors"]
            if errors_value is None:
                errors: tuple[StandardActionError, ...] = ()
            elif isinstance(errors_value, list):
                parsed_errors: list[StandardActionError] = []
                for error in errors_value:
                    if not isinstance(error, Mapping):
                        raise SalesforceClientError("malformed standard action error")
                    status_code = error.get("statusCode")
                    message = error.get("message")
                    fields_value = error.get("fields")
                    if not isinstance(status_code, str) or not isinstance(message, str):
                        raise SalesforceClientError("malformed standard action error fields")
                    if fields_value is None:
                        fields: tuple[str, ...] = ()
                    elif isinstance(fields_value, list) and all(
                        isinstance(field, str) for field in fields_value
                    ):
                        fields = tuple(fields_value)
                    else:
                        raise SalesforceClientError("malformed standard action error fields")
                    parsed_errors.append(StandardActionError(status_code, message, fields))
                errors = tuple(parsed_errors)
            else:
                raise SalesforceClientError("malformed standard action errors")
            output_value = item["outputValues"]
            if output_value is not None and not isinstance(output_value, Mapping):
                raise SalesforceClientError("malformed standard action outputValues")
            results.append(
                cls(
                    is_success=item["isSuccess"],
                    action_name=action_name,
                    errors=errors,
                    output_values=output_value,
                )
            )
        if not results:
            raise SalesforceClientError("standard action response contained no results")
        return tuple(results)


@dataclass(frozen=True)
class DeletedRecord:
    id: str
    deleted_date: datetime | None


@dataclass(frozen=True)
class GlobalDescribe:
    """Names of Salesforce objects available to the authenticated principal."""

    object_types: frozenset[str]

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> GlobalDescribe:
        objects_value = raw.get("sobjects")
        if not isinstance(objects_value, list):
            raise SalesforceClientError(
                "malformed global describe response: sobjects expected list, "
                f"got {type(objects_value).__name__}"
            )
        object_types: set[str] = set()
        for item in objects_value:
            if not isinstance(item, Mapping):
                raise SalesforceClientError(
                    "malformed global describe response: sobjects entry expected object, "
                    f"got {type(item).__name__}"
                )
            name = item.get("name")
            if not isinstance(name, str) or not name:
                raise SalesforceClientError(
                    "malformed global describe response: sobjects entry missing name"
                )
            object_types.add(name)
        return cls(object_types=frozenset(object_types))


@dataclass(frozen=True)
class FieldDescribe:
    """FLS/CRUD metadata for one field exposed by Salesforce Describe."""

    name: str
    createable: bool
    updateable: bool
    nillable: bool
    field_type: str | None
    relationship_name: str | None = None
    reference_to: tuple[str, ...] = ()


@dataclass(frozen=True)
class ObjectDescribe:
    """Provider-confirmed fields, queryability, and permissions for one object."""

    fields: frozenset[str]
    relationships: frozenset[str]
    field_metadata: Mapping[str, FieldDescribe]
    queryable: bool
    createable: bool
    updateable: bool

    def can_select(self, field: str) -> bool:
        if "." in field:
            relationship, _, child = field.partition(".")
            return relationship in self.relationships and bool(child)
        return field in self.fields

    def can_write(self, field: str, *, create: bool) -> bool:
        metadata = self.field_metadata.get(field)
        if metadata is None:
            return False
        return metadata.createable if create else metadata.updateable

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> ObjectDescribe:
        capabilities: dict[str, bool] = {}
        for key in ("queryable", "createable", "updateable"):
            value = raw.get(key)
            if not isinstance(value, bool):
                raise SalesforceClientError(
                    f"malformed object describe response: {key} expected boolean"
                )
            capabilities[key] = value
        fields_value = raw.get("fields")
        if not isinstance(fields_value, list):
            raise SalesforceClientError(
                "malformed object describe response: fields expected list, "
                f"got {type(fields_value).__name__}"
            )
        fields: set[str] = set()
        relationships: set[str] = set()
        metadata: dict[str, FieldDescribe] = {}
        for item in fields_value:
            if not isinstance(item, Mapping):
                raise SalesforceClientError(
                    "malformed object describe response: fields entry expected object, "
                    f"got {type(item).__name__}"
                )
            name = item.get("name")
            if not isinstance(name, str) or not name:
                raise SalesforceClientError(
                    "malformed object describe response: fields entry missing name"
                )
            fields.add(name)
            relationship_value = item.get("relationshipName")
            relationship_name = relationship_value if isinstance(relationship_value, str) else None
            if relationship_name:
                relationships.add(relationship_name)
            reference_value = item.get("referenceTo")
            reference_to = (
                tuple(value for value in reference_value if isinstance(value, str))
                if isinstance(reference_value, list) else ()
            )
            metadata[name] = FieldDescribe(
                name=name,
                createable=item.get("createable") is True,
                updateable=item.get("updateable") is True,
                nillable=item.get("nillable") is True,
                field_type=item.get("type") if isinstance(item.get("type"), str) else None,
                relationship_name=relationship_name,
                reference_to=reference_to,
            )
        return cls(
            fields=frozenset(fields),
            relationships=frozenset(relationships),
            field_metadata=metadata,
            **capabilities,
        )


@dataclass(frozen=True)
class DeletedResult:
    """Typed envelope of the /deleted endpoint response."""

    deleted_records: tuple[DeletedRecord, ...]
    earliest_date_available: datetime | None
    latest_date_covered: datetime | None
    next_records_url: str | None

    @classmethod
    def from_response(cls, raw: Mapping[str, object]) -> DeletedResult:
        records_value = raw.get("deletedRecords")
        if not isinstance(records_value, list):
            raise SalesforceClientError(
                "malformed deleted response: deletedRecords expected list, "
                f"got {type(records_value).__name__}"
            )
        records: list[DeletedRecord] = []
        for item in records_value:
            if not isinstance(item, Mapping):
                raise SalesforceClientError(
                    f"malformed deleted response: entry expected object, got {type(item).__name__}"
                )
            deleted_date_value = item.get("deletedDate")
            records.append(
                DeletedRecord(
                    id=_as_str(item.get("id"), "id"),
                    deleted_date=(
                        _parse_iso(deleted_date_value) if deleted_date_value is not None else None
                    ),
                )
            )
        next_url_value = raw.get("nextRecordsUrl")
        earliest = raw.get("earliestDateAvailable")
        latest = raw.get("latestDateCovered")
        return cls(
            deleted_records=tuple(records),
            earliest_date_available=(_parse_iso(earliest) if earliest is not None else None),
            latest_date_covered=_parse_iso(latest) if latest is not None else None,
            next_records_url=_as_str(next_url_value, "nextRecordsUrl")
            if next_url_value is not None
            else None,
        )


def _parse_iso(value: object) -> datetime:
    if not isinstance(value, str):
        raise SalesforceClientError(
            f"malformed timestamp in API response: expected string, got {type(value).__name__}"
        )
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as e:
        raise SalesforceClientError(f"malformed timestamp in API response: {value!r}") from e


def _normalize_next_records_path(next_records_url: str) -> tuple[str, dict[str, str]]:
    """Split a Salesforce next-records URL into a restful-relative path/params.

    ``simple_salesforce.restful`` prepends ``base_url`` (which already ends in
    ``/services/data/<version>/``), so passing the raw URL would duplicate the
    API path. Require the expected versioned prefix and return the remainder.
    """
    parsed = urlparse(next_records_url)
    prefix = f"/services/data/{API_VERSION}/"
    if not parsed.path.startswith(prefix):
        raise SalesforceClientError(
            f"unexpected Salesforce next-records URL: {next_records_url!r}"
        )
    relative = parsed.path[len(prefix) :]
    segments = relative.split("/")
    if (
        not relative
        or relative.startswith("/")
        or any(segment in {"", ".", ".."} for segment in segments)
    ):
        raise SalesforceClientError(
            f"unexpected Salesforce next-records URL: {next_records_url!r}"
        )
    params = dict(parse_qsl(parsed.query))
    return relative, params


def _normalize_next_query_path(next_records_url: str) -> tuple[str, dict[str, str]]:
    path, params = _normalize_next_records_path(next_records_url)
    if not (path.startswith("query/") or path.startswith("tooling/query/")):
        raise SalesforceClientError("unexpected Salesforce query pagination URL")
    return path, params


class SalesforceClient:
    """Async wrapper around simple-salesforce with typed responses and auth.

    Auth comes from a :class:`SalesforceAuth`: a static bearer token (quick
    test sessions) or JWT bearer auto-refresh (the connected-app flow used in
    production). In JWT mode the client mints short-lived access tokens itself
    and re-authenticates automatically on expiry/401.
    """

    def __init__(self, auth: SalesforceAuth, instance_url: str | None = None):
        self._auth = auth
        # Optional fallback instance URL (e.g. from the source config). JWT
        # mode usually learns the instance from the token response instead.
        self._fallback_instance_url = instance_url
        self._sf: Salesforce | None = None
        self._token: str | None = None
        self._token_instance_url: str | None = None
        self._token_expires_at: float = 0.0
        self._describe_cache: dict[str, ObjectDescribe] = {}

    @property
    def instance_url(self) -> str:
        return (
            self._token_instance_url or self._auth.instance_url or self._fallback_instance_url or ""
        )

    async def _ensure_session(self) -> Salesforce:
        """Return a session with a valid token, fetching one if needed."""
        if self._sf is not None:
            if self._auth.mode != AuthMode.JWT or not self._token_expiring_soon():
                return self._sf
            # Token approaching expiry: drop it and mint a fresh one.
            self._sf = None
            self._token = None
            self._describe_cache.clear()

        token, instance_url = await self._session_credentials()

        version = API_VERSION.lstrip("v")
        session = requests.Session()
        original_request = cast(Callable[..., requests.Response], session.request)

        def request_with_timeout(method: str, url: str, **kwargs: object) -> requests.Response:
            kwargs.setdefault("timeout", SALESFORCE_HTTP_TIMEOUT_SECONDS)
            return original_request(method, url, **kwargs)

        setattr(session, "request", request_with_timeout)
        self._sf = Salesforce(
            instance_url=instance_url,
            session_id=token,
            version=version,
            session=session,
        )
        # simple-salesforce always builds an https base_url; honor the exact
        # instance_url (https in production, http for mocks/dev instances).
        self._sf.base_url = f"{instance_url.rstrip('/')}/services/data/v{version}/"
        self._token = token
        self._token_instance_url = instance_url
        return self._sf

    async def _session_credentials(self) -> tuple[str, str]:
        """Return a valid (token, instance_url) pair for this auth mode."""
        if self._auth.mode == AuthMode.JWT:
            if self._token is not None and self._token_instance_url is not None:
                if not self._token_expiring_soon():
                    # Reuse the cached (just-refreshed) token.
                    return self._token, self._token_instance_url
                return await asyncio.to_thread(self._fetch_jwt_token)
            return await asyncio.to_thread(self._fetch_jwt_token)
        token = self._auth.access_token
        instance_url = self._auth.instance_url or self._fallback_instance_url
        if token is None:
            raise AuthenticationError("Missing access_token in credentials")
        if instance_url is None:
            raise AuthenticationError("Missing instance_url in credentials")
        return token, instance_url

    async def _refresh_expired_token(self) -> bool:
        """Fetch a fresh token on 401; returns True if one was minted."""
        if self._auth.mode != AuthMode.JWT:
            return False
        self._sf = None
        self._token = None
        self._token_expires_at = 0.0
        # The refreshed principal can have a different field-level security
        # profile, so describe results must be re-fetched.
        self._describe_cache.clear()
        await asyncio.to_thread(self._fetch_jwt_token)
        return True

    def _fetch_jwt_token(self) -> tuple[str, str]:
        """Exchange a signed assertion for an access token (JWT bearer flow)."""
        client_id = self._auth.client_id
        username = self._auth.username
        private_key = self._auth.private_key
        if not client_id or not username or not private_key:
            raise AuthenticationError("JWT credentials incomplete")
        now = int(time.time())
        payload = {
            "iss": client_id,
            "sub": username,
            "aud": self._auth.login_url,
            "iat": now,
            "exp": now + 300,
        }
        assertion = jwt.encode(payload, private_key, algorithm="RS256")
        response = requests.post(
            f"{self._auth.login_url.rstrip('/')}/services/oauth2/token",
            data={
                "grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                "assertion": assertion,
            },
            timeout=30,
            allow_redirects=False,
        )
        if response.status_code != 200:
            raise AuthenticationError(
                f"JWT token request failed ({response.status_code}): {response.text[:200]}"
            )
        try:
            body_raw: object = response.json()
        except ValueError as e:
            raise AuthenticationError("JWT token response was not valid JSON") from e
        body = _require_mapping(body_raw, "JWT token response")
        token = body.get("access_token")
        instance_url = body.get("instance_url")
        if not isinstance(token, str) or not token:
            raise AuthenticationError("JWT token response missing access_token")
        if not isinstance(instance_url, str) or not instance_url:
            raise AuthenticationError("JWT token response missing instance_url")
        issued_at = body.get("issued_at")
        expires_in = body.get("expires_in")
        default_refresh = 7200 if not isinstance(expires_in, int) else expires_in
        expires_at: float = float(now + default_refresh)
        if isinstance(issued_at, int) and issued_at > 0:
            expires_at = issued_at / 1000 + default_refresh
        self._token = token
        self._token_instance_url = instance_url
        self._token_expires_at = expires_at
        if (
            self._auth.mode == AuthMode.JWT
            and self._auth.login_url.startswith("https")
            and self._auth.private_key
        ):
            logger.info(
                "Minted fresh Salesforce access token for %s (expires in %ds)",
                self._auth.username,
                max(0, int(expires_at - time.time())),
            )
        return token, instance_url

    def _token_expiring_soon(self) -> bool:
        return (
            self._token is not None
            and self._token_expires_at > 0
            and time.time() >= self._token_expires_at - TOKEN_REFRESH_EARLY_SECONDS
        )

    async def session_credentials(self) -> tuple[str, str]:
        """Return a valid ``(access_token, instance_url)`` pair.

        Public view of the auth machinery so callers outside the sync flow
        (e.g. OAuth credential validation) can obtain a usable access token
        for both bearer and JWT credentials without duplicating the JWT
        bearer grant.
        """
        return await self._session_credentials()

    @with_retry(max_retries=3)
    async def query(self, soql: str) -> QueryResult:
        """Execute a SOQL query and return a typed result."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.query, soql)
        return QueryResult.from_response(_require_mapping(raw, "query"))

    @with_retry(max_retries=3)
    async def available_object_types(self) -> frozenset[str]:
        """Return objects exposed by the authenticated Salesforce edition/user."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.describe)
        return GlobalDescribe.from_response(
            _require_mapping(raw, "global describe")
        ).object_types

    @with_retry(max_retries=3)
    async def describe_object(self, object_type: str) -> ObjectDescribe:
        """Return fields and relationships the principal can actually query."""
        cached = self._describe_cache.get(object_type)
        if cached is not None:
            return cached
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.restful, f"sobjects/{object_type}/describe")
        describe = ObjectDescribe.from_response(_require_mapping(raw, "object describe"))
        self._describe_cache[object_type] = describe
        return describe

    @with_retry(max_retries=3)
    async def query_more(self, next_records_url: str) -> QueryResult:
        """Fetch the next page without trusting its URL authority."""
        sf = await self._ensure_session()
        path, params = _normalize_next_query_path(next_records_url)
        if not path.startswith("query/"):
            raise SalesforceClientError("unexpected Salesforce query pagination URL")
        raw = await asyncio.to_thread(sf.restful, path, params=params or None)
        return QueryResult.from_response(_require_mapping(raw, "query page"))

    @with_retry(max_retries=3)
    async def query_tooling(self, soql: str) -> QueryResult:
        """Execute a SOQL query through Salesforce's Tooling API."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.restful, "tooling/query/", params={"q": soql})
        return QueryResult.from_response(_require_mapping(raw, "tooling query"))

    @with_retry(max_retries=3)
    async def query_more_tooling(self, next_records_url: str) -> QueryResult:
        """Fetch the next page of a Tooling API query."""
        sf = await self._ensure_session()
        path, params = _normalize_next_query_path(next_records_url)
        if not path.startswith("tooling/query/"):
            raise SalesforceClientError("unexpected Salesforce Tooling pagination URL")
        raw = await asyncio.to_thread(sf.restful, path, params=params or None)
        return QueryResult.from_response(_require_mapping(raw, "tooling query page"))

    @with_retry(max_retries=3)
    async def get_deleted(self, object_type: str, start: datetime, end: datetime) -> DeletedResult:
        """List records deleted within [start, end]."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(
            sf.restful,
            f"sobjects/{object_type}/deleted",
            params={
                "start": _format_api_datetime(start),
                "end": _format_api_datetime(end),
            },
        )
        return DeletedResult.from_response(_require_mapping(raw, "deleted"))

    @with_retry(max_retries=3)
    async def get_deleted_more(self, next_records_url: str) -> DeletedResult:
        """Fetch the next page of a deleted-records result."""
        path, params = _normalize_next_records_path(next_records_url)
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.restful, path, params=params or None)
        return DeletedResult.from_response(_require_mapping(raw, "deleted page"))

    @with_retry(max_retries=0)
    async def create(self, object_type: str, data: Mapping[str, object]) -> str:
        """Create a record without replaying ambiguous 5xx failures."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(
            sf.restful,
            f"sobjects/{object_type}/",
            method="POST",
            data=json.dumps({k: v for k, v in data.items() if v is not None}),
        )
        record_id = raw.get("id") if isinstance(raw, Mapping) else None
        if not isinstance(record_id, str):
            raise SalesforceClientError(f"malformed create response for {object_type}: missing id")
        return record_id

    @with_retry(max_retries=3)
    async def update(self, object_type: str, record_id: str, data: Mapping[str, object]) -> None:
        """Update a record; PATCH is safe to retry."""
        sf = await self._ensure_session()
        await asyncio.to_thread(
            sf.restful,
            f"sobjects/{object_type}/{record_id}",
            method="PATCH",
            data=json.dumps(dict(data)),
        )

    @with_retry(max_retries=3)
    async def get_record(
        self, object_type: str, record_id: str, fields: tuple[str, ...]
    ) -> Mapping[str, object]:
        """Fetch a single record by id with the given fields."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(
            sf.restful,
            f"sobjects/{object_type}/{record_id}",
            params={"fields": ",".join(fields)},
        )
        if not isinstance(raw, Mapping):
            raise SalesforceClientError(f"malformed record response for {object_type} {record_id}")
        return raw

    @with_retry(max_retries=0)
    async def invoke_standard_action(
        self, action_name: str, payload: Mapping[str, object]
    ) -> tuple[StandardActionResult, ...]:
        """Invoke a named Salesforce standard action without retrying it.

        Standard actions can have side effects (email send in particular), so an
        ambiguous transport failure must be surfaced rather than replayed.
        """
        if not action_name.startswith("/") or ".." in action_name:
            raise SalesforceClientError("invalid Salesforce standard action path")
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(
            sf.restful,
            f"actions/standard/{action_name.lstrip('/').removeprefix('actions/standard/')}",
            method="POST",
            data=json.dumps(dict(payload)),
        )
        return StandardActionResult.from_response(raw)

    async def get_record_type_picklist_values(
        self, object_type: str, record_type_id: str, field_name: str
    ) -> tuple[str, ...]:
        """Read UI API picklist values for one record type and field."""
        path = (
            f"/ui-api/object-info/{object_type}/picklist-values/"
            f"{record_type_id}/{field_name}"
        )
        raw = _require_mapping(
            await self._raw_get_json(path, timeout=30), "UI API picklist"
        )
        values = raw.get("values")
        if not isinstance(values, list):
            raise SalesforceClientError("malformed UI API picklist response: values expected list")
        result: list[str] = []
        for item in values:
            if not isinstance(item, Mapping) or not isinstance(item.get("value"), str):
                raise SalesforceClientError("malformed UI API picklist value")
            if item.get("active") is not False:
                result.append(item["value"])
        return tuple(result)

    async def fetch_binary(
        self, path: str, *, max_bytes: int = MAX_BINARY_BYTES
    ) -> tuple[bytes, str | None, int | None]:
        """Fetch a binary Salesforce resource with typed HTTP errors."""
        if not path.startswith("/") or ".." in path:
            raise SalesforceClientError("invalid Salesforce binary resource path")
        token, instance_url = await self._session_credentials()

        def _fetch(current_token: str) -> requests.Response:
            try:
                return requests.get(
                    f"{instance_url.rstrip('/')}/services/data/{API_VERSION}{path}",
                    headers={"Authorization": f"Bearer {current_token}"},
                    timeout=60,
                    stream=True,
                )
            except requests.RequestException as exc:
                raise SalesforceClientError("Salesforce binary request failed") from exc

        response = await asyncio.to_thread(_fetch, token)
        if response.status_code == 401 and await self._refresh_expired_token():
            refreshed_token, _ = await self._session_credentials()
            response = await asyncio.to_thread(_fetch, refreshed_token)
        if response.status_code == 401:
            raise AuthenticationError("Invalid or expired access token")
        if response.status_code == 404:
            raise NotFoundError(path)
        if response.status_code == 403:
            raise ForbiddenError(path)
        if response.status_code >= 400:
            raise SalesforceClientError(
                f"Salesforce binary request failed ({response.status_code})"
            )
        content_length = _content_length(response)
        if content_length is not None and content_length > max_bytes:
            response.close()
            raise SalesforceClientError(
                "Salesforce binary response exceeds the configured size limit"
            )
        chunks: list[bytes] = []
        total = 0
        try:
            for chunk in response.iter_content(chunk_size=64 * 1024):
                total += len(chunk)
                if total > max_bytes:
                    raise SalesforceClientError(
                        "Salesforce binary response exceeds the configured size limit"
                    )
                chunks.append(chunk)
        finally:
            response.close()
        return b"".join(chunks), response.headers.get("Content-Type"), content_length or total

    async def _raw_get_json(self, path: str, *, timeout: float) -> object:
        if not path.startswith("/") or ".." in path:
            raise SalesforceClientError("invalid Salesforce resource path")
        token, instance_url = await self._session_credentials()

        def _fetch(current_token: str) -> requests.Response:
            try:
                return requests.get(
                    f"{instance_url.rstrip('/')}/services/data/{API_VERSION}{path}",
                    headers={"Authorization": f"Bearer {current_token}"},
                    timeout=timeout,
                )
            except requests.RequestException as exc:
                raise SalesforceClientError("Salesforce HTTP request failed") from exc

        response = await asyncio.to_thread(_fetch, token)
        if response.status_code == 401 and await self._refresh_expired_token():
            refreshed_token, _ = await self._session_credentials()
            response = await asyncio.to_thread(_fetch, refreshed_token)
        if response.status_code == 401:
            raise AuthenticationError("Invalid or expired access token")
        if response.status_code == 403:
            raise ForbiddenError("Insufficient permissions")
        if response.status_code == 404:
            raise NotFoundError(path)
        if response.status_code >= 400:
            raise SalesforceClientError(
                f"Salesforce HTTP request failed ({response.status_code}): {response.text[:200]}"
            )
        try:
            return response.json()
        except ValueError as exc:
            raise SalesforceClientError("Salesforce HTTP response was not valid JSON") from exc

    @with_retry(max_retries=3)
    async def test_connection(self) -> None:
        """Verify the token with an API endpoint independent of CRM objects."""
        sf = await self._ensure_session()
        raw = await asyncio.to_thread(sf.limits)
        _require_mapping(raw, "limits")


def _content_length(response: requests.Response) -> int | None:
    value = response.headers.get("Content-Length")
    if value is None:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def _format_api_datetime(value: datetime) -> str:
    return value.strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass(frozen=True)
class SalesforceUserIdentity:
    organization_id: str
    user_id: str
    username: str
    instance_url: str


def _verified_salesforce_host(value: str) -> str:
    parsed = urlparse(value)
    host = (parsed.hostname or "").lower().rstrip(".")
    if (
        parsed.scheme != "https"
        or parsed.username
        or parsed.password
        or parsed.port
        or parsed.query
        or parsed.fragment
        or parsed.path not in {"", "/"}
        or not (host.endswith(".salesforce.com") or host.endswith(".force.com"))
    ):
        raise SalesforceClientError("Invalid Salesforce instance URL")
    return host


def _verified_salesforce_login_url(value: str) -> str:
    parsed = urlparse(value)
    host = (parsed.hostname or "").lower().rstrip(".")
    if (
        parsed.scheme != "https"
        or parsed.username
        or parsed.password
        or parsed.port
        or parsed.query
        or parsed.fragment
        or parsed.path not in {"", "/"}
        or not (
            host in {"login.salesforce.com", "test.salesforce.com"}
            or host.endswith(".my.salesforce.com")
        )
    ):
        raise SalesforceClientError("Invalid Salesforce login URL")
    return f"https://{host}"


async def _verify_instance_url(
    instance_url: str, token: str, timeout: float
) -> str:
    host = _verified_salesforce_host(instance_url)
    root_url = f"https://{host}/services/data/{API_VERSION}/"

    def _fetch() -> Mapping[str, object]:
        response = requests.get(
            root_url,
            headers={"Authorization": f"Bearer {token}"},
            timeout=timeout,
            allow_redirects=False,
        )
        if response.status_code == 401:
            raise AuthenticationError("Invalid or expired access token")
        if response.status_code == 403:
            raise ForbiddenError("Salesforce API access denied")
        if response.status_code != 200:
            raise SalesforceClientError(
                f"Salesforce API root request failed ({response.status_code})"
            )
        return _require_mapping(response.json(), "API root")

    api_root = await asyncio.to_thread(_fetch)
    sobjects_url = api_root.get("sobjects")
    if not isinstance(sobjects_url, str):
        raise SalesforceClientError("Salesforce API root did not report its sobjects URL")
    parsed = urlparse(sobjects_url)
    if parsed.path != f"/services/data/{API_VERSION}/sobjects":
        raise SalesforceClientError("Salesforce API root reported an invalid sobjects URL")
    if parsed.query or parsed.fragment or parsed.username or parsed.password:
        raise SalesforceClientError("Salesforce API root reported an invalid sobjects URL")
    verified_host = _verified_salesforce_host(f"{parsed.scheme}://{parsed.netloc}")
    return f"https://{verified_host}"


async def fetch_user_identity(
    auth: SalesforceAuth, timeout: float = 15.0
) -> SalesforceUserIdentity:
    """Resolve org, principal, and provider-verified instance for this OAuth token."""
    login_url = _verified_salesforce_login_url(auth.login_url)
    if auth.instance_url:
        _verified_salesforce_host(auth.instance_url)
    client = SalesforceClient(auth)
    token, _instance_url = await client.session_credentials()

    def _fetch() -> Mapping[str, object]:
        response = requests.get(
            f"{login_url}/services/oauth2/userinfo",
            headers={"Authorization": f"Bearer {token}"},
            timeout=timeout,
            allow_redirects=False,
        )
        if response.status_code == 401:
            raise AuthenticationError("Invalid or expired access token")
        if response.status_code == 403:
            raise ForbiddenError("Salesforce userinfo access denied")
        if response.status_code != 200:
            raise SalesforceClientError(
                f"Salesforce userinfo request failed ({response.status_code})"
            )
        return _require_mapping(response.json(), "userinfo")

    userinfo = await asyncio.to_thread(_fetch)
    organization_id = userinfo.get("organization_id")
    user_id = userinfo.get("user_id")
    username = userinfo.get("preferred_username", userinfo.get("username"))
    if not auth.instance_url:
        raise SalesforceClientError("Salesforce credential did not report an instance URL")
    instance_url = await _verify_instance_url(auth.instance_url, token, timeout)
    if not isinstance(organization_id, str) or not organization_id:
        raise SalesforceClientError("Salesforce userinfo did not report an organization id")
    if not isinstance(user_id, str) or not user_id:
        raise SalesforceClientError("Salesforce userinfo did not report a user id")
    if not isinstance(username, str) or not username:
        raise SalesforceClientError("Salesforce userinfo did not report a username")
    return SalesforceUserIdentity(organization_id, user_id, username, instance_url)


async def fetch_organization_id(auth: SalesforceAuth, timeout: float = 15.0) -> str:
    """Resolve org ID only; OAuth setup credentials need not include userinfo claims."""
    login_url = _verified_salesforce_login_url(auth.login_url)
    if auth.instance_url:
        _verified_salesforce_host(auth.instance_url)
    if auth.mode == AuthMode.BEARER and auth.access_token is not None:
        token = auth.access_token
    else:
        client = SalesforceClient(auth)
        token, _instance_url = await client.session_credentials()

    def _fetch() -> Mapping[str, object]:
        response = requests.get(
            f"{login_url}/services/oauth2/userinfo",
            headers={"Authorization": f"Bearer {token}"},
            timeout=timeout,
            allow_redirects=False,
        )
        if response.status_code != 200:
            raise SalesforceClientError(
                f"Salesforce userinfo request failed ({response.status_code})"
            )
        return _require_mapping(response.json(), "userinfo")

    organization_id = (await asyncio.to_thread(_fetch)).get("organization_id")
    if not isinstance(organization_id, str) or not organization_id:
        raise SalesforceClientError("Salesforce userinfo did not report an organization id")
    return organization_id


def _require_mapping(raw: object, what: str) -> Mapping[str, object]:
    if not isinstance(raw, Mapping):
        raise SalesforceClientError(f"malformed {what} response: expected object")
    return raw
