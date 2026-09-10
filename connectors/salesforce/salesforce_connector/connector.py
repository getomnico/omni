"""Salesforce connector for Omni."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import tempfile
from collections.abc import Callable, Mapping
from dataclasses import dataclass, replace
from datetime import UTC, datetime, timedelta
from urllib.parse import urlparse

from fastapi.responses import JSONResponse
from omni_connector import (
    ActionDefinition,
    Connector,
    ConnectorManifest,
    OAuthCredentialFlow,
    OAuthCredentialReadyRequest,
    OAuthManifestConfig,
    OAuthScopeSet,
    OAuthSourceBinding,
    PersonSyncRecord,
    SearchOperator,
    StdioMcpServer,
    SyncContext,
    SyncMode,
)
from omni_connector.models import Source

from .actions import ACTION_DEFINITIONS, execute_action
from .client import (
    AuthenticationError,
    SalesforceClient,
    SalesforceClientError,
    fetch_organization_id,
)
from .config import (
    CHECKPOINT_INTERVAL,
    DELETION_RETENTION_DAYS,
    DELTA_OVERLAP_SECONDS,
    MAX_SHARE_SNAPSHOT_ENTRIES,
    PAGE_SIZE,
    PERMISSION_RECONCILIATION_INTERVAL_SECONDS,
    REALTIME_HEARTBEAT_SECONDS,
    SalesforceObjectConfig,
    SalesforceObjectName,
    SyncRunMode,
    enabled_object_configs,
    schema_fingerprint,
)
from .mappers import (
    RecordModel,
    attributes_for,
    generate_content,
    map_record_to_document,
)
from .models import (
    AccountRecord,
    CaseRecord,
    ContactRecord,
    GroupMemberRecord,
    GroupRecord,
    LeadRecord,
    ObjectState,
    OpportunityRecord,
    PeopleState,
    RecordCursor,
    RoleRecord,
    RunProgress,
    SalesforceAuth,
    SalesforceCheckpoint,
    SalesforceSourceConfig,
    ShareRecord,
    ShareSnapshot,
    TaskRecord,
    UserRecord,
    person_fingerprint,
)
from .pagination import (
    cursor_from_record,
    delta_scan_soql,
    full_scan_soql,
    iter_query_pages,
)
from .permissions import (
    RecordGrants,
    SalesforceDirectory,
    build_directory,
)

logger = logging.getLogger(__name__)

USER_ID_PREFIX = "005"

USER_FIELDS = (
    "Id",
    "Name",
    "FirstName",
    "LastName",
    "Email",
    "Title",
    "Department",
    "ManagerId",
    "UserRoleId",
    "IsActive",
    "EmployeeNumber",
    "SystemModstamp",
)
GROUP_FIELDS = ("Id", "Name", "Type", "RelatedId")
GROUP_MEMBER_FIELDS = ("Id", "GroupId", "UserOrGroupId")
ROLE_FIELDS = ("Id", "Name", "ParentRoleId")

# Group types whose membership is needed to resolve share rows. The
# Organization group (all users) is deliberately excluded: fail-closed
# visibility is configured explicitly via public_read_objects.
SYNCED_GROUP_TYPES = (
    "Public",
    "Queue",
    "Regular",
    "Role",
    "RoleAndSubordinates",
    "RoleAndSubordinatesInternal",
)

MCP_WORKSPACE_ENV = "OMNI_SALESFORCE_MCP_WORKSPACE"

# MCP tools are classified conservatively: only names matching a known
# read-only verb are treated as reads, everything else is a write so a
# read-scoped credential can never reach a write-capable tool.
MCP_READ_TOOL_PREFIXES = (
    "get_",
    "list_",
    "find_",
    "search_",
    "query_",
    "describe_",
    "read_",
    "fetch_",
    "show_",
    "retrieve_",
)
MCP_WRITE_TOOL_PREFIXES = (
    "create_",
    "update_",
    "delete_",
    "insert_",
    "upsert_",
    "remove_",
    "edit_",
    "publish_",
    "send_",
)

_RECORD_PARSERS: dict[SalesforceObjectName, Callable[[Mapping[str, object]], RecordModel]] = {
    SalesforceObjectName.ACCOUNT: AccountRecord.from_record,
    SalesforceObjectName.CONTACT: ContactRecord.from_record,
    SalesforceObjectName.OPPORTUNITY: OpportunityRecord.from_record,
    SalesforceObjectName.LEAD: LeadRecord.from_record,
    SalesforceObjectName.CASE: CaseRecord.from_record,
    SalesforceObjectName.TASK: TaskRecord.from_record,
}


@dataclass(frozen=True)
class ShareQueryPlan:
    """Validated SELECT plan for one object's share table."""

    share_object: str
    parent_field: str
    access_level_field: str
    fields: tuple[str, ...]


@dataclass(frozen=True)
class ResolvedObject:
    """A record object with the provider-confirmed fields it can be queried by."""

    config: SalesforceObjectConfig
    fields: tuple[str, ...]
    has_system_modstamp: bool
    share_plan: ShareQueryPlan | None


@dataclass(frozen=True)
class ShareSyncResult:
    grants_by_parent: dict[str, RecordGrants]
    changed_parents: dict[str, set[str]]
    snapshot: ShareSnapshot | None
    reconciliation_objects: frozenset[str]


class SalesforceConnector(Connector):
    """Salesforce CRM connector for Omni."""

    @property
    def name(self) -> str:
        return "salesforce"

    @property
    def display_name(self) -> str:
        return "Salesforce"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["salesforce"]

    @property
    def description(self) -> str:
        return "Index Salesforce accounts, contacts, opportunities, leads, cases and tasks"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental", "realtime"]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(operator="owner", attribute_key="owner_email", value_type="person"),
            SearchOperator(operator="status", attribute_key="status", value_type="text"),
            SearchOperator(operator="priority", attribute_key="priority", value_type="text"),
            SearchOperator(operator="stage", attribute_key="stage", value_type="text"),
            SearchOperator(operator="account", attribute_key="account_name", value_type="text"),
            SearchOperator(operator="industry", attribute_key="industry", value_type="text"),
            SearchOperator(operator="lead_source", attribute_key="lead_source", value_type="text"),
        ]

    @property
    def actions(self) -> list[ActionDefinition]:
        return list(ACTION_DEFINITIONS)

    async def get_manifest(self, connector_url: str) -> ConnectorManifest:
        """Reclassify discovered MCP tools so read authorization cannot reach writes."""
        manifest = await super().get_manifest(connector_url)
        manifest.actions = [self._classify_mcp_action(action) for action in manifest.actions]
        return manifest

    @staticmethod
    def _mcp_action_mode(tool_name: str) -> str:
        lowered = tool_name.lower()
        if lowered.startswith(MCP_WRITE_TOOL_PREFIXES):
            return "write"
        if lowered.startswith(MCP_READ_TOOL_PREFIXES):
            return "read"
        return "write"

    def _classify_mcp_action(self, action: ActionDefinition) -> ActionDefinition:
        if action.origin != "mcp":
            return action
        return action.model_copy(update={"mode": self._mcp_action_mode(action.name)})

    def oauth_config(self) -> OAuthManifestConfig | None:
        """Declare Salesforce's per-user OAuth/PKCE flow for MCP actions."""
        login_url = self._mcp_login_url("https://login.salesforce.com")
        return OAuthManifestConfig(
            provider="salesforce",
            auth_endpoint=f"{login_url}/services/oauth2/authorize",
            token_endpoint=f"{login_url}/services/oauth2/token",
            userinfo_endpoint=f"{login_url}/services/oauth2/userinfo",
            registration_endpoint=f"{login_url}/services/oauth2/register",
            registration_requires_initial_access_token=True,
            token_response_fields=["instance_url"],
            userinfo_email_field="email",
            identity_scopes=["openid", "email", "profile"],
            scopes={
                "salesforce": OAuthScopeSet(
                    read=["api", "offline_access"],
                    write=["api", "offline_access"],
                )
            },
            # Salesforce DCR is authenticated with an administrator-provided
            # initial access token and returns a confidential client secret.
            token_endpoint_auth_method="client_secret_post",
            issuer_source_config_key="login_url",
            client_config_provider_template="salesforce:{source_id}",
            pkce_required=True,
            grant_types=["authorization_code", "refresh_token"],
            validate_endpoint_urls=True,
            supports_org_oauth=False,
        )

    @property
    def mcp_server(self) -> StdioMcpServer:
        """Use Salesforce's official stdio MCP server."""
        return StdioMcpServer(command="omni-salesforce-mcp")

    @staticmethod
    def _credential_payload(credentials: Mapping[str, object]) -> Mapping[str, object]:
        """Accept both SDK raw credentials and action ServiceCredential envelopes."""
        nested = credentials.get("credentials")
        if isinstance(nested, Mapping):
            return nested
        return credentials

    @staticmethod
    def _mcp_login_url(value: object) -> str:
        url = value if isinstance(value, str) else "https://login.salesforce.com"
        parsed = urlparse(url)
        host = (parsed.hostname or "").lower().rstrip(".")
        allowed_host = (
            host in {"login.salesforce.com", "test.salesforce.com"}
            or host.endswith(".my.salesforce.com")
            or host.endswith(".sandbox.my.salesforce.com")
        )
        if (
            parsed.scheme != "https"
            or parsed.username
            or parsed.password
            or parsed.port
            or parsed.query
            or parsed.fragment
            or parsed.path not in {"", "/"}
            or not allowed_host
        ):
            raise ValueError("Salesforce MCP requires an HTTPS Salesforce login URL")
        return url.rstrip("/")

    @classmethod
    def _mcp_login_url_from_payload(cls, payload: Mapping[str, object]) -> str:
        login_url = payload.get("login_url")
        if not isinstance(login_url, str) or not login_url.strip():
            token_uri = payload.get("token_uri")
            if isinstance(token_uri, str):
                parsed = urlparse(token_uri)
                if parsed.scheme and parsed.netloc and parsed.path.endswith(
                    "/services/oauth2/token"
                ):
                    login_url = f"{parsed.scheme}://{parsed.netloc}"
        return cls._mcp_login_url(login_url or "https://login.salesforce.com")

    @staticmethod
    def _source_binding(config: Mapping[str, object]) -> dict[str, str]:
        """Read the stored provider binding, preferring the reserved
        `source_binding` key and falling back to the legacy top-level keys
        written by older web versions."""
        binding = config.get("source_binding")
        if isinstance(binding, Mapping) and all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in binding.items()
        ):
            return dict(binding)
        legacy: dict[str, str] = {}
        organization_id = config.get("organization_id")
        if isinstance(organization_id, str) and organization_id:
            legacy["organization_id"] = organization_id
        instance_url = config.get("instance_url")
        if isinstance(instance_url, str) and instance_url:
            legacy["instance_url"] = instance_url
        return legacy

    async def validate_oauth_credential(
        self,
        source: Source,
        credentials: dict[str, object],
        flow: OAuthCredentialFlow,
        metadata: dict[str, object] | None = None,
    ) -> OAuthSourceBinding | None:
        stored_binding = self._source_binding(source.config)
        expected_instance = stored_binding.get("instance_url")
        expected_instance = (
            expected_instance.strip().rstrip("/")
            if isinstance(expected_instance, str) and expected_instance.strip()
            else None
        )
        expected_org_id = stored_binding.get("organization_id")
        expected_org_id = (
            expected_org_id if isinstance(expected_org_id, str) and expected_org_id else None
        )

        # Organization identity only ever comes from provider-verified OAuth
        # userinfo: either the metadata captured during the OAuth exchange or a
        # fresh userinfo request with the supplied credential. A value carried
        # in the credential itself is an assertion to compare, never proof.
        actual_org_id: str | None = None
        metadata_org_id = (metadata or {}).get("organization_id")
        if isinstance(metadata_org_id, str) and metadata_org_id:
            actual_org_id = metadata_org_id
        if actual_org_id is None:
            actual_org_id = await self._organization_id_from_credential(credentials)

        asserted_org_id = credentials.get("organization_id")
        if (
            isinstance(asserted_org_id, str)
            and asserted_org_id
            and asserted_org_id != actual_org_id
        ):
            raise ValueError("Salesforce OAuth organization does not match the credential")

        binding: OAuthSourceBinding = {"organization_id": actual_org_id}

        if expected_instance is None and expected_org_id is None:
            return binding
        if expected_org_id is not None and actual_org_id != expected_org_id:
            raise ValueError("Salesforce OAuth organization does not match the source")

        actual_instance = credentials.get("instance_url")
        actual_instance = actual_instance if isinstance(actual_instance, str) else None
        if expected_instance is not None:
            if actual_instance is None:
                raise ValueError("Salesforce OAuth credential has no instance URL")
            try:
                expected_url = self._mcp_salesforce_url(expected_instance)
                actual_url = self._mcp_salesforce_url(actual_instance)
            except ValueError as exc:
                raise ValueError("Salesforce OAuth instance does not match the source") from exc
            if urlparse(expected_url).hostname != urlparse(actual_url).hostname:
                raise ValueError("Salesforce OAuth instance does not match the source")

        return binding

    async def _organization_id_from_credential(
        self, credentials: dict[str, object]
    ) -> str:
        try:
            auth = SalesforceAuth.from_mapping(self._credential_payload(credentials))
            return await fetch_organization_id(auth)
        except (SalesforceClientError, ValueError) as exc:
            raise ValueError(
                "Salesforce credential could not be verified against its Salesforce org"
            ) from exc

    def mcp_authentication_error(self, message: str) -> bool:
        """Recognize terminal Salesforce OAuth failures for MCP responses."""
        lowered = message.lower()
        return any(
            marker in lowered
            for marker in (
                "invalid_grant",
                "invalid session",
                "invalid access token",
                "authentication failed",
                "authentication required",
                "oauth organization",
                "access-token login failed",
                "oauth token",
                "credentials require",
                "missing credentials",
                "unsupported Salesforce MCP authentication",
                "unauthorized",
            )
        )

    def prepare_mcp_env(self, credentials: dict[str, object]) -> dict[str, str]:
        payload = self._credential_payload(credentials)
        auth = SalesforceAuth.from_mapping(payload)
        source_id = credentials.get("source_id") or credentials.get("_omni_source_id")
        user_id = credentials.get("user_id") or credentials.get("_omni_user_id")
        if isinstance(source_id, str) and source_id and isinstance(user_id, str) and user_id:
            source_id = f"{source_id}:{user_id}"
        if not isinstance(source_id, str) or not source_id:
            # This is only a fallback for direct SDK callers. Hashing the
            # canonical credential payload avoids putting secrets in a path.
            source_id = hashlib.sha256(
                json.dumps(
                    {"credentials": payload, "user_id": user_id},
                    sort_keys=True,
                    default=str,
                ).encode()
            ).hexdigest()
        env = {"OMNI_SALESFORCE_SOURCE_ID": source_id}
        if auth.mode.value == "jwt":
            assert auth.client_id and auth.private_key and auth.username
            env.update(
                {
                    "OMNI_SALESFORCE_AUTH_MODE": "jwt",
                    "SF_CLIENT_ID": auth.client_id,
                    "SF_PRIVATE_KEY": auth.private_key,
                    "SF_USERNAME": auth.username,
                    "SF_LOGIN_URL": self._mcp_login_url(auth.login_url),
                }
            )
        else:
            if not auth.access_token or not auth.instance_url:
                raise ValueError("Salesforce MCP OAuth credentials require an instance URL")
            env.update(
                {
                    "OMNI_SALESFORCE_AUTH_MODE": "access_token",
                    "SF_ACCESS_TOKEN": auth.access_token,
                    "SF_INSTANCE_URL": self._mcp_salesforce_url(auth.instance_url),
                    "SF_LOGIN_URL": self._mcp_login_url_from_payload(payload),
                }
            )
            organization_id = payload.get("organization_id")
            if isinstance(organization_id, str) and organization_id:
                env["SF_ORG_ID"] = organization_id

        # Create the marker only after all local credential validation has
        # succeeded; otherwise a rejected direct call could leak a temp file.
        status_fd, status_file = tempfile.mkstemp(prefix="omni-salesforce-auth-")
        os.close(status_fd)
        env["OMNI_MCP_AUTH_STATUS_FILE"] = status_file
        return env

    @staticmethod
    def _mcp_salesforce_url(value: str) -> str:
        parsed = urlparse(value)
        host = (parsed.hostname or "").lower().rstrip(".")
        allowed_host = host.endswith(".salesforce.com") or host.endswith(".force.com")
        if (
            parsed.scheme != "https"
            or parsed.username
            or parsed.password
            or parsed.port
            or parsed.query
            or parsed.fragment
            or parsed.path not in {"", "/"}
            or not allowed_host
        ):
            raise ValueError("Salesforce MCP requires an HTTPS Salesforce instance URL")
        return value.rstrip("/")

    @staticmethod
    def _mcp_workspace() -> str:
        return os.environ.get(MCP_WORKSPACE_ENV) or os.path.join(os.getcwd(), "salesforce-mcp")

    def prepare_mcp_tool_arguments(
        self, action: str, arguments: Mapping[str, object]
    ) -> dict[str, object]:
        prepared = dict(arguments)
        directory = prepared.get("directory")
        if directory is None:
            return prepared
        if not isinstance(directory, str) or not directory:
            raise ValueError(f"Salesforce MCP tool {action} requires a directory path")

        workspace = os.path.realpath(self._mcp_workspace())
        os.makedirs(workspace, mode=0o700, exist_ok=True)
        candidate = os.path.realpath(
            directory if os.path.isabs(directory) else os.path.join(workspace, directory)
        )
        # The official Salesforce MCP schema asks for a sandbox-container path
        # that is not mounted here, and a caller-supplied path may attempt to
        # escape via traversal or symlinks. Confine every tool to the
        # connector-owned workspace instead of trusting the requested path.
        if candidate != workspace and not candidate.startswith(workspace + os.sep):
            candidate = workspace
        os.makedirs(candidate, mode=0o700, exist_ok=True)
        prepared["directory"] = candidate
        return prepared

    async def bootstrap_mcp(self, credentials: dict[str, object]) -> None:
        # Sync has only the org JWT credential. MCP catalogs must be discovered
        # with a user OAuth credential, never with the sync credential.
        if not credentials.get("user_id") and not credentials.get("_omni_user_id"):
            logger.info("Skipping MCP bootstrap without a user OAuth credential")
            return
        await super().bootstrap_mcp(credentials)

    async def oauth_credential_ready(self, request: OAuthCredentialReadyRequest) -> bool:
        if request.provider != "salesforce" or not request.user_id:
            return False
        credentials = dict(request.credentials)
        credentials["_omni_source_id"] = request.source_id
        credentials["_omni_user_id"] = request.user_id
        adapter = self.mcp_adapter
        if adapter is None:
            return False
        try:
            auth = self._prepare_mcp_auth(credentials)
            await adapter.discover(**auth)
            return True
        except Exception:
            logger.warning("Salesforce MCP user catalog bootstrap failed", exc_info=True)
            return False

    async def execute_action(
        self,
        action: str,
        params: Mapping[str, object],
        credentials: Mapping[str, object],
        source: Source | None = None,
        actor_email: str | None = None,
    ) -> JSONResponse:
        return await execute_action(action, params, self._credential_payload(credentials))

    async def sync(
        self,
        source_config: Mapping[str, object],
        credentials: Mapping[str, object],
        checkpoint: Mapping[str, object] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            auth = SalesforceAuth.from_mapping(credentials)
        except ValueError as e:
            await ctx.fail(str(e))
            return

        try:
            config = SalesforceSourceConfig.from_mapping(source_config)
            config.validate()
        except ValueError as e:
            await ctx.fail(str(e))
            return
        client = SalesforceClient(auth, instance_url=config.instance_url)

        try:
            await client.test_connection()
        except AuthenticationError as e:
            await ctx.fail(f"Authentication failed: {e}")
            return
        except SalesforceClientError as e:
            await ctx.fail(f"Connection test failed: {e}")
            return

        logger.info(
            "Starting Salesforce %s sync for %s",
            ctx.sync_mode.value,
            client.instance_url,
        )

        # Schema fingerprint: when the synced field/object/visibility set
        # changes, saved watermarks no longer cover everything the index
        # expects, so a full resync is forced.
        fingerprint = schema_fingerprint(config.enabled_objects, config.public_read_objects)
        run_checkpoint = SalesforceCheckpoint.from_mapping(checkpoint)
        if ctx.connector_state.get("schema_fingerprint") != fingerprint:
            logger.info("Schema fingerprint changed; forcing full resync")
            run_checkpoint = SalesforceCheckpoint()
            await ctx.save_connector_state({"schema_fingerprint": fingerprint})

        try:
            if ctx.sync_mode == SyncMode.REALTIME:
                await self._realtime_sync(client, config, run_checkpoint, ctx)
                return

            completed = await self._run_scheduled_sync(client, config, run_checkpoint, ctx)
            if ctx.is_cancelled():
                # A cancelled run must not promote its partially covered
                # checkpoint to the source baseline.
                logger.info("Sync cancelled before completion; not publishing checkpoint")
                return
            await ctx.complete(checkpoint=completed.without_progress().to_json())
            logger.info(
                "Sync completed: %d scanned, %d emitted",
                ctx.documents_scanned,
                ctx.documents_emitted,
            )
        except AuthenticationError as e:
            logger.error("Authentication error during sync: %s", e)
            await ctx.fail(f"Authentication failed: {e}")
        except SalesforceClientError as e:
            logger.error("Salesforce API error during sync: %s", e)
            await ctx.fail(str(e))
        except Exception as e:
            logger.exception("Sync failed with unexpected error")
            await ctx.fail(str(e))

    # -- checkpoint helpers -------------------------------------------------

    @staticmethod
    def _with_progress(checkpoint: SalesforceCheckpoint, **changes: object) -> SalesforceCheckpoint:
        progress = checkpoint.progress
        if progress is None:
            raise RuntimeError("checkpoint has no run progress")
        return replace(checkpoint, progress=replace(progress, **changes))  # type: ignore[arg-type]

    @staticmethod
    def _with_object_state(
        checkpoint: SalesforceCheckpoint, object_name: str, state: ObjectState
    ) -> SalesforceCheckpoint:
        return replace(
            checkpoint, objects={**checkpoint.objects, object_name: state}
        )

    @staticmethod
    async def _heartbeat(ctx: SyncContext) -> None:
        """Persist the current run checkpoint to keep the manager from marking
        the run stale during provider-only work."""
        await ctx.save_checkpoint(ctx.checkpoint)

    def _resume_progress(
        self,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        mode: SyncRunMode,
        window_end: datetime,
    ) -> RunProgress:
        progress = checkpoint.progress
        if (
            ctx.is_resume
            and progress is not None
            and progress.run_id == ctx.sync_run_id
            and progress.mode == mode
        ):
            return progress
        return RunProgress(
            run_id=ctx.sync_run_id,
            mode=mode,
            window_end=window_end.isoformat(),
            started_at=datetime.now(UTC).isoformat(),
        )

    # -- capability discovery ----------------------------------------------

    async def _resolve_objects(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        ctx: SyncContext,
    ) -> tuple[tuple[ResolvedObject, ...], frozenset[str]]:
        """Resolve a typed query plan for every enabled record and share object."""
        available = await client.available_object_types()
        requested = enabled_object_configs(config.enabled_objects)
        resolved: list[ResolvedObject] = []
        for item in requested:
            if item.name.value not in available:
                logger.warning(
                    "Skipping Salesforce object unavailable to this principal: %s",
                    item.name.value,
                )
                if config.enabled_objects:
                    await ctx.emit_error(
                        f"{item.name.value}:*",
                        "Salesforce object is not available to this principal",
                    )
                continue

            describe = await client.describe_object(item.name.value)
            fields = tuple(field for field in item.all_fields() if describe.can_select(field))
            required = {"Id"}
            if not required.issubset(fields):
                logger.warning("Skipping %s: Id field is not accessible", item.name.value)
                await ctx.emit_error(
                    f"{item.name.value}:*", "Salesforce object Id field is not accessible"
                )
                continue
            has_system_modstamp = "SystemModstamp" in fields
            if not has_system_modstamp:
                logger.warning(
                    "Salesforce %s does not expose SystemModstamp; falling back to "
                    "full scans for this object",
                    item.name.value,
                )
                await ctx.emit_error(
                    f"{item.name.value}:*",
                    "SystemModstamp is unavailable; using full-only reconciliation",
                )

            share_plan: ShareQueryPlan | None = None
            if config.sync_shares and item.share_object is not None:
                share_plan = await self._resolve_share_plan(client, item, available, ctx)

            resolved.append(
                ResolvedObject(
                    config=item,
                    fields=fields,
                    has_system_modstamp=has_system_modstamp,
                    share_plan=share_plan,
                )
            )
        return tuple(resolved), available

    async def _resolve_share_plan(
        self,
        client: SalesforceClient,
        item: SalesforceObjectConfig,
        available: frozenset[str],
        ctx: SyncContext,
    ) -> ShareQueryPlan | None:
        share_object = item.share_object
        parent_field = item.share_parent_field
        access_level_field = item.share_access_level_field
        if share_object is None or parent_field is None or access_level_field is None:
            return None
        if share_object not in available:
            logger.info("Skipping unavailable Salesforce share object %s", share_object)
            return None
        describe = await client.describe_object(share_object)
        fields = ("Id", parent_field, "UserOrGroupId", access_level_field, "RowCause")
        missing = [field for field in fields if not describe.can_select(field)]
        if missing:
            logger.warning(
                "Skipping %s shares: fields not exposed by Salesforce: %s",
                share_object,
                ", ".join(missing),
            )
            await ctx.emit_error(
                f"{item.name.value}:*",
                f"Share object {share_object} is missing fields: {', '.join(missing)}",
            )
            return None
        return ShareQueryPlan(
            share_object=share_object,
            parent_field=parent_field,
            access_level_field=access_level_field,
            fields=fields,
        )

    async def _selectable_fields(
        self,
        client: SalesforceClient,
        object_type: str,
        candidates: tuple[str, ...],
    ) -> tuple[str, ...]:
        """Narrow a SELECT list to fields the principal can actually query."""
        describe = await client.describe_object(object_type)
        selectable = tuple(field for field in candidates if describe.can_select(field))
        missing = tuple(field for field in candidates if field not in selectable)
        if missing:
            logger.warning(
                "Skipping Salesforce fields unavailable on %s: %s",
                object_type,
                ", ".join(missing),
            )
        if "Id" not in selectable:
            logger.warning("Skipping %s: Id field is not accessible", object_type)
            return ()
        return selectable

    # -- scheduled sync -----------------------------------------------------

    async def _run_scheduled_sync(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> SalesforceCheckpoint:
        """One-shot sync: people, shares, records, deletes."""
        mode = (
            SyncRunMode.INCREMENTAL
            if ctx.sync_mode == SyncMode.INCREMENTAL
            else SyncRunMode.FULL
        )
        window_end = datetime.now(UTC)
        progress = self._resume_progress(checkpoint, ctx, mode, window_end)
        checkpoint = replace(checkpoint, progress=progress)
        # Persist the run-scoped checkpoint before any provider work, so an
        # immediate restart receives an unambiguous run checkpoint.
        await ctx.save_checkpoint(checkpoint.to_json())

        objects, available = await self._resolve_objects(client, config, ctx)

        directory, people_state = await self._sync_people(
            client, config, ctx, checkpoint.people, available
        )
        checkpoint = replace(checkpoint, people=people_state)
        if ctx.is_cancelled():
            return checkpoint

        share_result = await self._sync_shares(
            client, objects, directory, ctx, checkpoint.share_snapshot
        )
        checkpoint = replace(checkpoint, share_snapshot=share_result.snapshot)
        if share_result.reconciliation_objects:
            current_progress = checkpoint.progress
            if current_progress is not None:
                checkpoint = self._with_progress(
                    checkpoint,
                    full_reconciliation=tuple(
                        sorted(
                            set(current_progress.full_reconciliation)
                            | share_result.reconciliation_objects
                        )
                    ),
                )
        if ctx.is_cancelled():
            return checkpoint

        checkpoint = await self._sync_objects(
            client=client,
            objects=objects,
            directory=directory,
            share_grants=share_result.grants_by_parent,
            changed_parents=share_result.changed_parents,
            source_config=config,
            checkpoint=checkpoint,
            ctx=ctx,
            incremental=mode == SyncRunMode.INCREMENTAL,
            window_end=window_end,
        )
        return replace(checkpoint, synced_at=datetime.now(UTC).isoformat())

    async def _sync_people(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        ctx: SyncContext,
        previous: PeopleState | None,
        available_objects: frozenset[str],
    ) -> tuple[SalesforceDirectory, PeopleState]:
        """Query users/groups/roles and emit person and group-membership events."""
        users: list[UserRecord] = []
        groups: list[GroupRecord] = []
        group_members: list[GroupMemberRecord] = []
        roles: list[RoleRecord] = []

        if config.sync_users and "User" in available_objects:
            fields = await self._selectable_fields(client, "User", USER_FIELDS)
            if fields:
                if "IsActive" not in fields:
                    logger.warning(
                        "Salesforce User.IsActive is unavailable; lifecycle state is "
                        "unknown and memberships cannot be resolved safely"
                    )
                    await ctx.emit_error(
                        "User:*",
                        "User.IsActive is unavailable; treating active state as unknown",
                    )
                async for page in iter_query_pages(
                    client, f"SELECT {', '.join(fields)} FROM User"
                ):
                    users.extend(UserRecord.from_record(r) for r in page.records)
                    await self._heartbeat(ctx)
        if config.sync_groups:
            if "Group" in available_objects:
                fields = await self._selectable_fields(client, "Group", GROUP_FIELDS)
                if fields:
                    group_types = ", ".join(f"'{value}'" for value in SYNCED_GROUP_TYPES)
                    async for page in iter_query_pages(
                        client,
                        f"SELECT {', '.join(fields)} FROM Group "
                        f"WHERE Type IN ({group_types})",
                    ):
                        groups.extend(GroupRecord.from_record(r) for r in page.records)
                        await self._heartbeat(ctx)
            if "GroupMember" in available_objects:
                fields = await self._selectable_fields(
                    client, "GroupMember", GROUP_MEMBER_FIELDS
                )
                if fields:
                    async for page in iter_query_pages(
                        client, f"SELECT {', '.join(fields)} FROM GroupMember"
                    ):
                        group_members.extend(
                            GroupMemberRecord.from_record(r) for r in page.records
                        )
                        await self._heartbeat(ctx)
            if "UserRole" in available_objects:
                fields = await self._selectable_fields(client, "UserRole", ROLE_FIELDS)
                if fields:
                    async for page in iter_query_pages(
                        client, f"SELECT {', '.join(fields)} FROM UserRole"
                    ):
                        roles.extend(RoleRecord.from_record(r) for r in page.records)
                        await self._heartbeat(ctx)

        directory = build_directory(users, groups, group_members, roles)
        snapshot = self._people_state(directory, users)

        await self._emit_people(directory, users, snapshot, previous, ctx)
        return directory, snapshot

    def _people_state(
        self, directory: SalesforceDirectory, users: list[UserRecord]
    ) -> PeopleState:
        user_fingerprints: dict[str, str] = {}
        active_emails: set[str] = set()
        for user in users:
            user_fingerprints[user.id] = person_fingerprint(user)
            if user.email and user.is_active is not False:
                active_emails.add(user.email)
        memberships: dict[str, tuple[str, ...]] = {}
        group_emails: set[str] = set()
        for group_email, member_emails, _name in directory.group_memberships():
            memberships[group_email] = tuple(sorted(member_emails))
            group_emails.add(group_email)
        return PeopleState(
            user_fingerprints=user_fingerprints,
            active_emails=frozenset(active_emails),
            group_emails=frozenset(group_emails),
            memberships=memberships,
        )

    async def _emit_people(
        self,
        directory: SalesforceDirectory,
        users: list[UserRecord],
        snapshot: PeopleState,
        previous: PeopleState | None,
        ctx: SyncContext,
    ) -> None:
        changed_users = (
            users
            if previous is None
            else [
                user
                for user in users
                if snapshot.user_fingerprints.get(user.id)
                != previous.user_fingerprints.get(user.id)
            ]
        )
        for user in changed_users:
            if not user.email:
                continue
            if user.is_active is False:
                await ctx.emit_person_deleted(user.email)
            else:
                await ctx.emit_person_sync(
                    PersonSyncRecord(
                        external_id=user.id,
                        email=user.email,
                        display_name=user.name,
                        given_name=user.first_name,
                        surname=user.last_name,
                        job_title=user.title,
                        department=user.department,
                        manager_external_id=user.manager_id,
                        employee_id=user.employee_number,
                        source_updated_at=(
                            user.system_modstamp.isoformat()
                            if user.system_modstamp is not None
                            else None
                        ),
                    )
                )

        if previous is not None:
            for email in sorted(previous.active_emails - snapshot.active_emails):
                await ctx.emit_person_deleted(email)

        for group_email, member_emails, name in directory.group_memberships():
            members = tuple(sorted(member_emails))
            if previous is not None and previous.memberships.get(group_email) == members:
                continue
            await ctx.emit_group_membership(
                group_email=group_email,
                member_emails=list(members),
                group_name=name,
            )

        if previous is not None:
            for removed in sorted(previous.group_emails - snapshot.group_emails):
                # A group that disappeared must revoke its memberships rather
                # than leaving the last known membership in place forever.
                await ctx.emit_group_membership(
                    group_email=removed,
                    member_emails=[],
                    group_name=None,
                )

    async def _sync_shares(
        self,
        client: SalesforceClient,
        objects: tuple[ResolvedObject, ...],
        directory: SalesforceDirectory,
        ctx: SyncContext,
        previous: ShareSnapshot | None,
    ) -> ShareSyncResult:
        """Refresh share rows, resolve grants, and diff against committed state."""
        grants_by_parent: dict[str, RecordGrants] = {}
        changed_parents: dict[str, set[str]] = {}
        snapshot_grants: dict[str, dict[str, str]] = {}
        reconciliation: set[str] = set()
        previous_grants = (
            previous.grants if previous is not None and not previous.oversized else {}
        )
        total_entries = 0

        for obj in objects:
            plan = obj.share_plan
            if plan is None:
                continue
            if ctx.is_cancelled():
                return ShareSyncResult(grants_by_parent, changed_parents, previous, frozenset())
            object_grants: dict[str, RecordGrants] = {}
            soql = (
                f"SELECT {', '.join(plan.fields)} FROM {plan.share_object} "
                "WHERE RowCause != 'Owner' ORDER BY Id"
            )
            try:
                async for page in iter_query_pages(client, soql):
                    for raw in page.records:
                        share = ShareRecord.from_record(
                            raw, plan.parent_field, plan.access_level_field
                        )
                        grants = directory.share_grants((share,))
                        if not grants.users and not grants.groups:
                            continue
                        object_grants[share.parent_id] = object_grants.get(
                            share.parent_id, RecordGrants()
                        ).merge(grants)
                    await self._heartbeat(ctx)
            except SalesforceClientError as e:
                logger.warning("Failed to sync %s shares: %s", plan.share_object, e)
                await ctx.emit_error(f"{obj.config.name}:*", f"Failed to fetch shares: {e}")
                # Keep the previous snapshot for this object so a transient
                # failure does not silently reset diff state.
                if previous is not None and plan.share_object in previous.grants:
                    snapshot_grants[plan.share_object] = previous.grants[plan.share_object]
                    total_entries += len(previous.grants[plan.share_object])
                continue

            fingerprint_map = {
                parent_id: grants.fingerprint() for parent_id, grants in object_grants.items()
            }
            snapshot_grants[plan.share_object] = fingerprint_map
            total_entries += len(fingerprint_map)
            for parent_id, grants in object_grants.items():
                grants_by_parent[parent_id] = grants

            prior = previous_grants.get(plan.share_object, {})
            changed = {
                parent_id
                for parent_id in set(prior) | set(fingerprint_map)
                if prior.get(parent_id) != fingerprint_map.get(parent_id)
            }
            if changed:
                changed_parents[obj.config.name] = changed

        if total_entries > MAX_SHARE_SNAPSHOT_ENTRIES:
            # Persisting an unbounded per-parent snapshot is not safe; fall back
            # to periodic full permission reconciliation.
            logger.warning(
                "Salesforce share snapshot exceeds %d entries; using periodic "
                "full permission reconciliation",
                MAX_SHARE_SNAPSHOT_ENTRIES,
            )
            due = self._reconciliation_due(previous)
            if due:
                reconciliation = {
                    obj.config.name for obj in objects if obj.share_plan is not None
                }
            snapshot = ShareSnapshot(
                grants={},
                captured_at=(
                    datetime.now(UTC).isoformat()
                    if due or previous is None or previous.captured_at is None
                    else previous.captured_at
                ),
                oversized=True,
            )
            return ShareSyncResult(
                grants_by_parent, {}, snapshot, frozenset(reconciliation)
            )

        snapshot = ShareSnapshot(
            grants=snapshot_grants,
            captured_at=datetime.now(UTC).isoformat(),
            oversized=False,
        )
        return ShareSyncResult(grants_by_parent, changed_parents, snapshot, frozenset())

    @staticmethod
    def _reconciliation_due(previous: ShareSnapshot | None) -> bool:
        if previous is None or not previous.oversized or previous.captured_at is None:
            return True
        try:
            captured = datetime.fromisoformat(previous.captured_at)
        except ValueError:
            return True
        return (
            datetime.now(UTC) - captured
        ).total_seconds() >= PERMISSION_RECONCILIATION_INTERVAL_SECONDS

    # -- record + deletion passes ------------------------------------------

    async def _sync_objects(
        self,
        *,
        client: SalesforceClient,
        objects: tuple[ResolvedObject, ...],
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        changed_parents: dict[str, set[str]],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        incremental: bool,
        window_end: datetime,
        tolerate_errors: bool = False,
    ) -> SalesforceCheckpoint:
        for obj in objects:
            if ctx.is_cancelled():
                return checkpoint
            progress = checkpoint.progress
            if progress is None:
                raise RuntimeError("checkpoint has no run progress")
            try:
                if obj.config.name not in progress.records_completed:
                    checkpoint = await self._sync_object_records(
                        client=client,
                        obj=obj,
                        directory=directory,
                        share_grants=share_grants,
                        source_config=source_config,
                        checkpoint=checkpoint,
                        ctx=ctx,
                        incremental=incremental,
                        window_end=window_end,
                    )
                    if ctx.is_cancelled():
                        return checkpoint
                progress = checkpoint.progress
                if progress is None:
                    raise RuntimeError("checkpoint has no run progress")
                if obj.config.name not in progress.deletions_completed:
                    checkpoint = await self._sync_object_deletions(
                        client=client,
                        obj=obj,
                        directory=directory,
                        share_grants=share_grants,
                        source_config=source_config,
                        checkpoint=checkpoint,
                        ctx=ctx,
                        window_end=window_end,
                    )
                    if ctx.is_cancelled():
                        return checkpoint
                checkpoint = self._commit_object_boundary(checkpoint, obj, window_end)
            except Exception as e:
                if not tolerate_errors:
                    raise
                logger.warning(
                    "Realtime pass failed for %s: %s; retaining committed boundary",
                    obj.config.name,
                    e,
                )
                await ctx.emit_error(f"{obj.config.name}:*", f"Realtime poll failed: {e}")
                continue
            await ctx.save_checkpoint(checkpoint.to_json())

        if incremental and changed_parents:
            checkpoint = await self._emit_changed_parents(
                client=client,
                objects=objects,
                changed_parents=changed_parents,
                directory=directory,
                share_grants=share_grants,
                source_config=source_config,
                checkpoint=checkpoint,
                ctx=ctx,
            )
        return checkpoint

    def _commit_object_boundary(
        self,
        checkpoint: SalesforceCheckpoint,
        obj: ResolvedObject,
        window_end: datetime,
    ) -> SalesforceCheckpoint:
        """Advance the record watermark only after records and deletions succeeded."""
        state = checkpoint.state_for(obj.config.name)
        watermark = state.watermark
        if obj.has_system_modstamp:
            watermark = (
                window_end - timedelta(seconds=DELTA_OVERLAP_SECONDS)
            ).isoformat()
        return self._with_object_state(
            checkpoint,
            obj.config.name,
            ObjectState(watermark=watermark, deletion_through=state.deletion_through),
        )

    async def _sync_object_records(
        self,
        *,
        client: SalesforceClient,
        obj: ResolvedObject,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        incremental: bool,
        window_end: datetime,
    ) -> SalesforceCheckpoint:
        config = obj.config
        progress = checkpoint.progress
        if progress is None:
            raise RuntimeError("checkpoint has no run progress")
        state = checkpoint.state_for(config.name)

        force_full = config.name in progress.full_reconciliation
        delta = (
            incremental
            and obj.has_system_modstamp
            and state.watermark is not None
            and not force_full
        )
        if delta and state.watermark is not None:
            window_start = datetime.fromisoformat(state.watermark) - timedelta(
                seconds=DELTA_OVERLAP_SECONDS
            )
        else:
            window_start = None

        cursor = progress.record_cursor if progress.current_object == config.name else None
        if delta:
            # A malformed or partial delta cursor cannot prove where the scan
            # stopped; restart the fixed window from the committed boundary.
            if cursor is not None and not cursor.is_delta_ready:
                cursor = None
        elif cursor is not None and cursor.last_id is None:
            cursor = None

        parser = _RECORD_PARSERS[config.name]
        emitted_since_checkpoint = 0
        while True:
            if ctx.is_cancelled():
                return checkpoint
            if delta:
                assert window_start is not None
                soql = delta_scan_soql(
                    config.name, obj.fields, cursor, window_start, window_end
                )
            else:
                soql = full_scan_soql(config.name, obj.fields, cursor)
            page = await client.query(soql)
            for raw in page.records:
                record = parser(raw)
                await self._emit_record(
                    client=client,
                    config=config,
                    record=record,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=source_config,
                    ctx=ctx,
                    emit_updated=incremental,
                )
                cursor = cursor_from_record(raw)
                emitted_since_checkpoint += 1
                if emitted_since_checkpoint >= CHECKPOINT_INTERVAL:
                    checkpoint = self._with_progress(
                        checkpoint, current_object=config.name, record_cursor=cursor
                    )
                    await ctx.save_checkpoint(checkpoint.to_json())
                    emitted_since_checkpoint = 0

            if ctx.is_cancelled():
                return checkpoint
            if len(page.records) < PAGE_SIZE:
                break

        current_progress = checkpoint.progress
        if current_progress is None:
            raise RuntimeError("checkpoint has no run progress")
        checkpoint = self._with_progress(
            checkpoint,
            current_object=None,
            record_cursor=None,
            records_completed=tuple(
                sorted(set(current_progress.records_completed) | {config.name})
            ),
        )
        await ctx.save_checkpoint(checkpoint.to_json())
        logger.info(
            "Finished scanning %s (%s)",
            config.name,
            "delta" if delta else "full",
        )
        return checkpoint

    async def _sync_object_deletions(
        self,
        *,
        client: SalesforceClient,
        obj: ResolvedObject,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
        window_end: datetime,
    ) -> SalesforceCheckpoint:
        """Emit tombstones for records deleted since the committed boundary."""
        config = obj.config
        state = checkpoint.state_for(config.name)

        bounded = True
        if state.deletion_through is not None:
            base = datetime.fromisoformat(state.deletion_through)
        elif state.watermark is not None:
            base = datetime.fromisoformat(state.watermark)
        else:
            base = window_end - timedelta(days=DELETION_RETENTION_DAYS)
            bounded = False
        requested_start = (
            base - timedelta(seconds=DELTA_OVERLAP_SECONDS) if bounded else base
        )

        result = await client.get_deleted(config.name, requested_start, window_end)
        earliest = result.earliest_date_available
        if (
            bounded
            and earliest is not None
            and earliest
            > requested_start + timedelta(seconds=DELTA_OVERLAP_SECONDS)
        ):
            # Deletions between the committed boundary and the provider's
            # retention start cannot be covered. Do not silently clamp past
            # them: force a full record reconciliation and surface the gap.
            logger.warning(
                "Salesforce deletion retention for %s starts at %s, after the "
                "committed boundary %s; forcing full reconciliation",
                config.name,
                earliest.isoformat(),
                requested_start.isoformat(),
            )
            await ctx.emit_error(
                f"{config.name}:*",
                "Deletion retention window no longer covers the committed boundary; "
                "full reconciliation required",
            )
            checkpoint = await self._reemit_all_records(
                client=client,
                obj=obj,
                directory=directory,
                share_grants=share_grants,
                source_config=source_config,
                checkpoint=checkpoint,
                ctx=ctx,
            )
            requested_start = earliest

        latest = result.latest_date_covered
        current = result
        while True:
            if ctx.is_cancelled():
                return checkpoint
            for deleted in current.deleted_records:
                await ctx.emit_deleted(f"{config.name}:{deleted.id}")
            if current.latest_date_covered is not None:
                latest = current.latest_date_covered
            if current.next_records_url is None:
                break
            current = await client.get_deleted_more(current.next_records_url)

        boundary = state.deletion_through
        if latest is not None and latest >= requested_start:
            # Advance only to provider-confirmed coverage; anything the provider
            # has not covered is picked up by the next pass.
            boundary = latest.isoformat()
        checkpoint = self._with_object_state(
            checkpoint,
            config.name,
            ObjectState(watermark=state.watermark, deletion_through=boundary),
        )
        current_progress = checkpoint.progress
        if current_progress is None:
            raise RuntimeError("checkpoint has no run progress")
        checkpoint = self._with_progress(
            checkpoint,
            deletions_completed=tuple(
                sorted(set(current_progress.deletions_completed) | {config.name})
            ),
        )
        await ctx.save_checkpoint(checkpoint.to_json())
        return checkpoint

    async def _reemit_all_records(
        self,
        *,
        client: SalesforceClient,
        obj: ResolvedObject,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> SalesforceCheckpoint:
        """Re-emit every current record during a forced full reconciliation."""
        cursor: RecordCursor | None = None
        while True:
            if ctx.is_cancelled():
                return checkpoint
            page = await client.query(full_scan_soql(obj.config.name, obj.fields, cursor))
            for raw in page.records:
                record = _RECORD_PARSERS[obj.config.name](raw)
                await self._emit_record(
                    client=client,
                    config=obj.config,
                    record=record,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=source_config,
                    ctx=ctx,
                    emit_updated=True,
                )
            if page.records:
                cursor = cursor_from_record(page.records[-1])
            if len(page.records) < PAGE_SIZE:
                break
        await ctx.save_checkpoint(checkpoint.to_json())
        return checkpoint

    async def _emit_changed_parents(
        self,
        *,
        client: SalesforceClient,
        objects: tuple[ResolvedObject, ...],
        changed_parents: dict[str, set[str]],
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> SalesforceCheckpoint:
        """Re-emit records whose sharing changed without a parent modstamp bump."""
        for obj in objects:
            ids = changed_parents.get(obj.config.name)
            if not ids:
                continue
            parser = _RECORD_PARSERS[obj.config.name]
            batch: list[str] = []
            for parent_id in sorted(ids):
                batch.append(parent_id)
                if len(batch) < 200:
                    continue
                checkpoint = await self._emit_parent_batch(
                    client=client,
                    obj=obj,
                    parser=parser,
                    ids=batch,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=source_config,
                    checkpoint=checkpoint,
                    ctx=ctx,
                )
                batch = []
                if ctx.is_cancelled():
                    return checkpoint
            if batch:
                checkpoint = await self._emit_parent_batch(
                    client=client,
                    obj=obj,
                    parser=parser,
                    ids=batch,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=source_config,
                    checkpoint=checkpoint,
                    ctx=ctx,
                )
        return checkpoint

    async def _emit_parent_batch(
        self,
        *,
        client: SalesforceClient,
        obj: ResolvedObject,
        parser: Callable[[Mapping[str, object]], RecordModel],
        ids: list[str],
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> SalesforceCheckpoint:
        in_clause = ", ".join(f"'{record_id}'" for record_id in ids)
        soql = (
            f"SELECT {', '.join(obj.fields)} FROM {obj.config.name} "
            f"WHERE Id IN ({in_clause})"
        )
        page = await client.query(soql)
        for raw in page.records:
            record = parser(raw)
            await self._emit_record(
                client=client,
                config=obj.config,
                record=record,
                directory=directory,
                share_grants=share_grants,
                source_config=source_config,
                ctx=ctx,
                emit_updated=True,
            )
        await ctx.save_checkpoint(checkpoint.to_json())
        return checkpoint

    async def _emit_record(
        self,
        *,
        client: SalesforceClient,
        config: SalesforceObjectConfig,
        record: RecordModel,
        directory: SalesforceDirectory,
        share_grants: dict[str, RecordGrants],
        source_config: SalesforceSourceConfig,
        ctx: SyncContext,
        emit_updated: bool,
    ) -> None:
        await ctx.increment_scanned()
        owner_id = record.owner_id
        owner_email = (
            directory.email_for_user(owner_id)
            if isinstance(owner_id, str) and owner_id.startswith(USER_ID_PREFIX)
            else None
        )
        grants = directory.owner_grants(owner_id, source_config.grant_access_using_hierarchies)
        share_grant = share_grants.get(record.id)
        if share_grant is not None:
            grants = grants.merge(share_grant)
        # Visibility is fail-closed: only objects explicitly configured as
        # public-read are visible to everyone.
        public = config.name in source_config.public_read_objects
        permissions = grants.to_permissions(public)

        content = generate_content(config.name, record)
        content_id = await ctx.content_storage.save(content, "text/plain")
        attributes = attributes_for(config.name, record, owner_email)
        document = map_record_to_document(
            object_type=config.name,
            record=record,
            content_id=content_id,
            instance_url=client.instance_url,
            owner_email=owner_email,
            permissions=permissions,
            attributes=attributes,
        )
        if emit_updated:
            await ctx.emit_updated(document)
        else:
            await ctx.emit(document)

    # -- realtime -----------------------------------------------------------

    async def _realtime_sync(
        self,
        client: SalesforceClient,
        config: SalesforceSourceConfig,
        checkpoint: SalesforceCheckpoint,
        ctx: SyncContext,
    ) -> None:
        """Long-lived polling sync. The connector-manager supervises this slot
        and restarts it if it dies; it returns only when cancelled."""
        objects, available = await self._resolve_objects(client, config, ctx)
        poll_seconds = max(config.realtime_poll_seconds, 10)
        people_interval = timedelta(seconds=max(poll_seconds * 10, 300))

        # Persist run-scoped progress before any provider work so a restart gets
        # an unambiguous run checkpoint and idle heartbeats never write a
        # progress-less checkpoint.
        started_at = datetime.now(UTC)
        checkpoint = replace(
            checkpoint,
            progress=self._resume_progress(
                checkpoint, ctx, SyncRunMode.INCREMENTAL, started_at
            ),
        )
        await ctx.save_checkpoint(checkpoint.to_json())

        directory: SalesforceDirectory | None = None
        people_state = checkpoint.people
        share_snapshot = checkpoint.share_snapshot
        share_grants: dict[str, RecordGrants] = {}
        reconcile_objects: set[str] = set()
        last_people_refresh: datetime | None = None
        failures = 0

        async def refresh_people_and_shares(now: datetime, *, propagate: bool) -> None:
            nonlocal directory, people_state, share_snapshot, share_grants
            nonlocal last_people_refresh, checkpoint
            directory, people_state = await self._sync_people(
                client, config, ctx, people_state, available
            )
            checkpoint = replace(checkpoint, people=people_state)
            await ctx.save_checkpoint(checkpoint.to_json())
            if ctx.is_cancelled():
                return
            result = await self._sync_shares(client, objects, directory, ctx, share_snapshot)
            share_snapshot = result.snapshot
            share_grants = result.grants_by_parent
            checkpoint = replace(checkpoint, share_snapshot=share_snapshot)
            await ctx.save_checkpoint(checkpoint.to_json())
            # An oversized share snapshot cannot be diffed, so the next poll
            # must fully re-emit the affected objects to reconcile permissions.
            reconcile_objects.update(result.reconciliation_objects)
            if propagate and result.changed_parents and not ctx.is_cancelled():
                checkpoint = await self._emit_changed_parents(
                    client=client,
                    objects=objects,
                    changed_parents=result.changed_parents,
                    directory=directory,
                    share_grants=share_grants,
                    source_config=config,
                    checkpoint=checkpoint,
                    ctx=ctx,
                )
            last_people_refresh = now

        if not any(
            checkpoint.state_for(obj.config.name).watermark is not None for obj in objects
        ):
            # No committed baseline yet: run a full pass so polling has
            # watermarks to work from. This checkpoint stays run-scoped; a
            # realtime run never completes(), so it cannot overwrite a
            # concurrently completed scheduled checkpoint.
            logger.info("Realtime sync: no watermarks, running baseline full sync")
            baseline_end = datetime.now(UTC)
            await ctx.save_checkpoint(checkpoint.to_json())
            await refresh_people_and_shares(baseline_end, propagate=False)
            if ctx.is_cancelled() or directory is None:
                return
            checkpoint = await self._sync_objects(
                client=client,
                objects=objects,
                directory=directory,
                share_grants=share_grants,
                changed_parents={},
                source_config=config,
                checkpoint=checkpoint,
                ctx=ctx,
                incremental=False,
                window_end=baseline_end,
            )
            await ctx.save_checkpoint(checkpoint.to_json())
            # The baseline full pass already re-emitted every record.
            reconcile_objects.clear()

        while True:
            if ctx.is_cancelled():
                return
            now = datetime.now(UTC)

            if directory is None or (
                last_people_refresh is not None
                and now - last_people_refresh >= people_interval
            ):
                await refresh_people_and_shares(now, propagate=True)
                if ctx.is_cancelled():
                    return

            # Each poll is an independent bounded pass: reset the pass
            # completion markers but keep committed per-object boundaries.
            checkpoint = replace(
                checkpoint,
                progress=RunProgress(
                    run_id=ctx.sync_run_id,
                    mode=SyncRunMode.INCREMENTAL,
                    window_end=now.isoformat(),
                    started_at=now.isoformat(),
                    full_reconciliation=tuple(sorted(reconcile_objects)),
                ),
            )
            assert directory is not None
            try:
                checkpoint = await self._sync_objects(
                    client=client,
                    objects=objects,
                    directory=directory,
                    share_grants=share_grants,
                    changed_parents={},
                    source_config=config,
                    checkpoint=checkpoint,
                    ctx=ctx,
                    incremental=True,
                    window_end=now,
                    tolerate_errors=True,
                )
                await ctx.save_checkpoint(checkpoint.to_json())
                reconcile_objects.clear()
                failures = 0
                delay = poll_seconds
            except Exception as e:
                logger.warning("Realtime poll failed: %s", e)
                await ctx.emit_error("*", f"Realtime poll failed: {e}")
                failures += 1
                delay = min(poll_seconds * (2**failures), 900)

            if ctx.is_cancelled():
                return
            await self._sleep_with_heartbeat(delay, checkpoint, ctx)

    async def _sleep_with_heartbeat(
        self, delay: float, checkpoint: SalesforceCheckpoint, ctx: SyncContext
    ) -> None:
        remaining = delay
        while remaining > 0:
            if ctx.is_cancelled():
                return
            chunk = min(remaining, float(REALTIME_HEARTBEAT_SECONDS))
            await asyncio.sleep(chunk)
            remaining -= chunk
            if ctx.is_cancelled():
                return
            if remaining > 0:
                await ctx.save_checkpoint(checkpoint.to_json())
