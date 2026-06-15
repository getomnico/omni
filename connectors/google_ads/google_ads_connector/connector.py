"""Main Google Ads connector."""

from __future__ import annotations

import csv
import io
import logging
from datetime import UTC, datetime
from typing import Any, cast

from fastapi.responses import JSONResponse, Response
from omni_connector import Connector, SearchOperator, SyncContext, SyncMode
from omni_connector.models import (
    ActionDefinition,
    ActionResponse,
    OAuthManifestConfig,
    OAuthScopeSet,
)

from .client import (
    GoogleAdsApiError,
    GoogleAdsClient,
    GoogleAdsConnectorError,
    InMemoryGoogleAdsClient,
)
from .config import GOOGLE_ADS_SCOPE, GoogleAdsCredentials, GoogleAdsSourceConfig
from .mappers import map_row_to_document, render_content, strip_metrics
from .models import (
    CHANGE_STATUS_QUERY_TEMPLATE,
    REPORT_RESOURCE_ALLOWLIST,
    SYNC_QUERIES,
)

logger = logging.getLogger(__name__)

CHECKPOINT_EVERY = 100


class GoogleAdsConnector(Connector):
    """Google Ads connector for Omni."""

    @property
    def name(self) -> str:
        return "google_ads"

    @property
    def display_name(self) -> str:
        return "Google Ads"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["google_ads"]

    @property
    def description(self) -> str:
        return (
            "Index Google Ads account structure and run live campaign analysis reports"
        )

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def search_operators(self) -> list[SearchOperator]:
        return [
            SearchOperator(
                operator="customer", attribute_key="customer_id", value_type="text"
            ),
            SearchOperator(
                operator="campaign", attribute_key="campaign_id", value_type="text"
            ),
            SearchOperator(
                operator="ad_group", attribute_key="ad_group_id", value_type="text"
            ),
            SearchOperator(
                operator="status", attribute_key="status", value_type="text"
            ),
            SearchOperator(
                operator="channel", attribute_key="channel_type", value_type="text"
            ),
            SearchOperator(
                operator="entity", attribute_key="entity_type", value_type="text"
            ),
            SearchOperator(operator="label", attribute_key="labels", value_type="text"),
        ]

    def oauth_config(self) -> OAuthManifestConfig | None:
        return OAuthManifestConfig(
            provider="google_ads",
            auth_endpoint="https://accounts.google.com/o/oauth2/v2/auth",
            token_endpoint="https://oauth2.googleapis.com/token",
            userinfo_endpoint="https://www.googleapis.com/oauth2/v3/userinfo",
            userinfo_email_field="email",
            identity_scopes=["email", "profile"],
            scopes={
                "google_ads": OAuthScopeSet(
                    read=[GOOGLE_ADS_SCOPE],
                    write=[GOOGLE_ADS_SCOPE],
                )
            },
            extra_auth_params={"access_type": "offline", "prompt": "consent"},
            scope_separator=" ",
        )

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="run_gaql_query",
                description=(
                    "Run a live Google Ads GAQL query for analysis. Returns structured JSON rows."
                ),
                mode="read",
                source_types=["google_ads"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "customer_id": {"type": "string"},
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "default": 1000, "maximum": 10000},
                    },
                    "required": ["customer_id", "query"],
                },
            ),
            ActionDefinition(
                name="export_gaql_report_csv",
                description="Run a live Google Ads GAQL query and return a CSV export.",
                mode="read",
                source_types=["google_ads"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "customer_id": {"type": "string"},
                        "query": {"type": "string"},
                        "limit": {
                            "type": "integer",
                            "default": 10000,
                            "maximum": 50000,
                        },
                    },
                    "required": ["customer_id", "query"],
                },
            ),
            ActionDefinition(
                name="export_gaql_report_xlsx",
                description="Run a live Google Ads GAQL query and return an XLSX export.",
                mode="read",
                source_types=["google_ads"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "customer_id": {"type": "string"},
                        "query": {"type": "string"},
                        "limit": {
                            "type": "integer",
                            "default": 10000,
                            "maximum": 50000,
                        },
                    },
                    "required": ["customer_id", "query"],
                },
            ),
            ActionDefinition(
                name="get_account_summary",
                description=(
                    "Fetch live customer/campaign structure and recent performance summary rows."
                ),
                mode="read",
                source_types=["google_ads"],
                input_schema={
                    "type": "object",
                    "properties": {
                        "customer_id": {"type": "string"},
                        "date_range": {"type": "string", "default": "LAST_30_DAYS"},
                    },
                    "required": ["customer_id"],
                },
            ),
            ActionDefinition(
                name="get_recommendations",
                description="Fetch live Google Ads recommendations for a customer.",
                mode="read",
                source_types=["google_ads"],
                input_schema={
                    "type": "object",
                    "properties": {"customer_id": {"type": "string"}},
                    "required": ["customer_id"],
                },
            ),
        ]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        try:
            raw_creds = credentials.get("credentials", credentials)
            merged_creds = (
                {**credentials, **raw_creds}
                if isinstance(raw_creds, dict)
                else credentials
            )
            creds = GoogleAdsCredentials.parse(merged_creds)
            config = GoogleAdsSourceConfig.parse(source_config, merged_creds)
        except ValueError as exc:
            await ctx.fail(str(exc))
            return

        if not config.sync_enabled:
            await ctx.complete(new_state=state or {})
            return

        client = self._make_client(creds, config, source_config)
        state = state or {}

        try:
            if ctx.sync_mode == SyncMode.INCREMENTAL:
                await self._incremental_sync(client, config, state, ctx)
            else:
                await self._full_sync(client, config, state, ctx)
        except GoogleAdsConnectorError as exc:
            logger.exception("Google Ads sync failed")
            await ctx.fail(str(exc))
        except Exception as exc:
            logger.exception("Unexpected Google Ads sync failure")
            await ctx.fail(str(exc))

    def _make_client(
        self,
        creds: GoogleAdsCredentials,
        config: GoogleAdsSourceConfig,
        source_config: dict[str, Any],
    ) -> GoogleAdsClient:
        if isinstance(source_config.get("mock_data"), dict):
            return InMemoryGoogleAdsClient(source_config["mock_data"])
        return GoogleAdsClient(creds, login_customer_id=config.login_customer_id)

    async def _full_sync(
        self,
        client: GoogleAdsClient,
        config: GoogleAdsSourceConfig,
        state: dict[str, Any],
        ctx: SyncContext,
    ) -> None:
        checkpoint = state.get("checkpoint", {}) if ctx.is_resume else {}
        completed = set(checkpoint.get("completed", []))
        scanned_since_checkpoint = 0

        for customer_id in sorted(config.customer_ids):
            for entity_type in config.entity_types:
                key = f"{customer_id}:{entity_type}"
                if key in completed:
                    continue
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return
                await self._sync_entity_type(client, customer_id, entity_type, ctx)
                completed.add(key)
                scanned_since_checkpoint += 1
                if scanned_since_checkpoint >= 1:
                    await ctx.save_checkpoint(
                        {
                            **state,
                            "checkpoint": {
                                "mode": "full",
                                "completed": sorted(completed),
                                "current_customer_id": customer_id,
                                "current_entity_type": entity_type,
                            },
                        }
                    )
                    scanned_since_checkpoint = 0

        now = datetime.now(UTC).isoformat()
        await ctx.complete(
            new_state={
                "last_successful_full_sync_at": now,
                "last_successful_sync_at": now,
                "customer_ids": config.customer_ids,
                "entity_types": config.entity_types,
                "connector_version": self.version,
            }
        )

    async def _sync_entity_type(
        self,
        client: GoogleAdsClient,
        customer_id: str,
        entity_type: str,
        ctx: SyncContext,
    ) -> None:
        query = SYNC_QUERIES.get(entity_type)
        if not query:
            return
        logger.info("Syncing Google Ads %s for customer %s", entity_type, customer_id)
        try:
            async for row in client.search_stream(customer_id, query):
                if ctx.is_cancelled():
                    return
                await ctx.increment_scanned()
                try:
                    content = render_content(entity_type, customer_id, row)
                    content_id = await ctx.content_storage.save(content, "text/plain")
                    doc = map_row_to_document(
                        entity_type=entity_type,
                        customer_id=customer_id,
                        row=row,
                        content_id=content_id,
                    )
                    await ctx.emit(doc)
                except Exception as exc:
                    logger.warning("Failed to map Google Ads row: %s", exc)
                    await ctx.emit_error(
                        f"google_ads:{customer_id}:{entity_type}:unknown", str(exc)
                    )
        except GoogleAdsApiError as exc:
            await ctx.emit_error(f"google_ads:{customer_id}:{entity_type}:*", str(exc))

    async def _incremental_sync(
        self,
        client: GoogleAdsClient,
        config: GoogleAdsSourceConfig,
        state: dict[str, Any],
        ctx: SyncContext,
    ) -> None:
        since = state.get("last_successful_incremental_sync_at") or state.get(
            "last_successful_sync_at"
        )
        if not since:
            await self._full_sync(client, config, state, ctx)
            return

        checkpoint = state.get("checkpoint", {}) if ctx.is_resume else {}
        completed_customers = set(checkpoint.get("completed_customers", []))
        now = datetime.now(UTC).isoformat()

        for customer_id in sorted(config.customer_ids):
            if customer_id in completed_customers:
                continue
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return
            changed = await self._changed_resource_types(
                client, customer_id, since, ctx
            )
            entity_types = [e for e in config.entity_types if e in changed]
            if not entity_types:
                entity_types = []
            for entity_type in entity_types:
                await self._sync_entity_type(client, customer_id, entity_type, ctx)
            completed_customers.add(customer_id)
            await ctx.save_checkpoint(
                {
                    **state,
                    "checkpoint": {
                        "mode": "incremental",
                        "since": since,
                        "last_processed_change_time": now,
                        "completed_customers": sorted(completed_customers),
                    },
                }
            )

        await ctx.complete(
            new_state={
                **state,
                "checkpoint": {},
                "last_successful_incremental_sync_at": now,
                "last_successful_sync_at": now,
                "customer_ids": config.customer_ids,
                "entity_types": config.entity_types,
                "connector_version": self.version,
            }
        )

    async def _changed_resource_types(
        self,
        client: GoogleAdsClient,
        customer_id: str,
        since: str,
        ctx: SyncContext,
    ) -> set[str]:
        query = CHANGE_STATUS_QUERY_TEMPLATE.format(since=since.replace("'", ""))
        resource_map = {
            "campaign": "campaign",
            "campaign_budget": "campaign_budget",
            "ad_group": "ad_group",
            "ad_group_ad": "ad_group_ad",
            "asset": "asset",
            "ad_group_criterion": "keyword_view",
            "user_list": "user_list",
            "conversion_action": "conversion_action",
        }
        changed: set[str] = set()
        try:
            async for row in client.search(customer_id, query):
                await ctx.increment_scanned()
                change_status = (
                    row.get("change_status") or row.get("changeStatus") or row
                )
                resource_type = str(
                    change_status.get("resource_type")
                    or change_status.get("resourceType")
                    or ""
                ).lower()
                if resource_type in resource_map:
                    changed.add(resource_map[resource_type])
        except GoogleAdsApiError as exc:
            await ctx.emit_error(f"google_ads:{customer_id}:change_status", str(exc))
        return changed

    async def execute_action(  # type: ignore[override]
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> JSONResponse | Response:
        try:
            raw_creds = credentials.get("credentials", credentials)
            merged_creds = (
                {**credentials, **raw_creds}
                if isinstance(raw_creds, dict)
                else credentials
            )
            creds = GoogleAdsCredentials.parse(merged_creds)
            raw_source_config = params.get("source_config")
            source_config = cast(
                dict[str, Any],
                raw_source_config if isinstance(raw_source_config, dict) else {},
            )
            cfg = GoogleAdsSourceConfig.parse(
                {"customer_ids": [params.get("customer_id")], **source_config},
                merged_creds,
            )
            client = self._make_client(creds, cfg, source_config)
        except Exception as exc:
            return ActionResponse.failure(str(exc)).to_response(status_code=400)

        try:
            if action == "run_gaql_query":
                return await self._action_run_gaql(client, params)
            if action == "export_gaql_report_csv":
                return await self._action_export_csv(client, params)
            if action == "export_gaql_report_xlsx":
                return await self._action_export_xlsx(client, params)
            if action == "get_account_summary":
                return await self._action_account_summary(client, params)
            if action == "get_recommendations":
                return await self._action_recommendations(client, params)
        except Exception as exc:
            logger.exception("Google Ads action failed")
            return ActionResponse.failure(str(exc)).to_response(status_code=500)
        return ActionResponse.not_supported(action).to_response(status_code=404)

    async def _action_run_gaql(
        self, client: GoogleAdsClient, params: dict[str, Any]
    ) -> Response:
        customer_id, query, limit = _action_query_params(params)
        validation_error = validate_gaql_for_action(query)
        if validation_error:
            return ActionResponse.failure(validation_error).to_response(status_code=400)
        rows = await client.run_gaql(customer_id, query, limit=limit)
        return ActionResponse.success(
            {"rows": rows, "row_count": len(rows)}
        ).to_response()

    async def _action_export_csv(
        self, client: GoogleAdsClient, params: dict[str, Any]
    ) -> Response:
        customer_id, query, limit = _action_query_params(params, default_limit=10000)
        validation_error = validate_gaql_for_action(query)
        if validation_error:
            return ActionResponse.failure(validation_error).to_response(status_code=400)
        rows = await client.run_gaql(customer_id, query, limit=limit)
        csv_text = rows_to_csv(rows)
        return Response(
            content=csv_text,
            media_type="text/csv; charset=utf-8",
            headers={
                "content-disposition": 'attachment; filename="google-ads-report.csv"'
            },
        )

    async def _action_export_xlsx(
        self, client: GoogleAdsClient, params: dict[str, Any]
    ) -> Response:
        customer_id, query, limit = _action_query_params(params, default_limit=10000)
        validation_error = validate_gaql_for_action(query)
        if validation_error:
            return ActionResponse.failure(validation_error).to_response(status_code=400)
        rows = await client.run_gaql(customer_id, query, limit=limit)
        content = rows_to_xlsx(rows)
        return Response(
            content=content,
            media_type="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            headers={
                "content-disposition": 'attachment; filename="google-ads-report.xlsx"'
            },
        )

    async def _action_account_summary(
        self, client: GoogleAdsClient, params: dict[str, Any]
    ) -> Response:
        customer_id = _require_customer_id(params)
        date_range = str(params.get("date_range") or "LAST_30_DAYS")
        query = f"""
            SELECT
              campaign.id,
              campaign.name,
              campaign.status,
              campaign.advertising_channel_type,
              metrics.impressions,
              metrics.clicks,
              metrics.cost_micros,
              metrics.conversions,
              metrics.conversions_value
            FROM campaign
            WHERE segments.date DURING {date_range}
            ORDER BY metrics.cost_micros DESC
        """
        rows = await client.run_gaql(customer_id, query, limit=1000)
        return ActionResponse.success(
            {"customer_id": customer_id, "date_range": date_range, "rows": rows}
        ).to_response()

    async def _action_recommendations(
        self, client: GoogleAdsClient, params: dict[str, Any]
    ) -> Response:
        customer_id = _require_customer_id(params)
        rows = await client.run_gaql(
            customer_id, SYNC_QUERIES["recommendation"], limit=1000
        )
        return ActionResponse.success(
            {"customer_id": customer_id, "recommendations": rows}
        ).to_response()


def _require_customer_id(params: dict[str, Any]) -> str:
    customer_id = str(params.get("customer_id") or "").replace("-", "").strip()
    if not customer_id:
        raise ValueError("Missing customer_id")
    return customer_id


def _action_query_params(
    params: dict[str, Any], default_limit: int = 1000
) -> tuple[str, str, int]:
    customer_id = _require_customer_id(params)
    query = str(params.get("query") or "").strip()
    if not query:
        raise ValueError("Missing query")
    limit = min(max(int(params.get("limit") or default_limit), 1), 50000)
    return customer_id, query, limit


def validate_gaql_for_action(query: str) -> str | None:
    lowered = " ".join(query.lower().split())
    if not lowered.startswith("select ") or " from " not in lowered:
        return "Only SELECT GAQL queries are supported"
    forbidden = [" mutate ", " insert ", " update ", " delete ", ";", "--", "/*"]
    if any(token in lowered for token in forbidden):
        return "Query contains unsupported tokens"
    resource = lowered.split(" from ", 1)[1].split()[0]
    if resource not in REPORT_RESOURCE_ALLOWLIST and resource not in SYNC_QUERIES:
        return f"Unsupported GAQL resource: {resource}"
    # Field allowlist is intentionally advisory for custom GAQL: reject obvious unsafe resources,
    # but allow Google Ads to validate evolving field compatibility.
    return None


def rows_to_csv(rows: list[dict[str, Any]]) -> str:
    flattened = [_flatten(strip_metrics(row)) for row in rows]
    fieldnames = sorted({key for row in flattened for key in row})
    output = io.StringIO()
    writer = csv.DictWriter(output, fieldnames=fieldnames)
    writer.writeheader()
    for row in flattened:
        writer.writerow(row)
    return output.getvalue()


def rows_to_xlsx(rows: list[dict[str, Any]]) -> bytes:
    from openpyxl import Workbook  # type: ignore[import-untyped]

    flattened = [_flatten(strip_metrics(row)) for row in rows]
    fieldnames = sorted({key for row in flattened for key in row}) or ["result"]
    wb = Workbook()
    ws = wb.active
    ws.title = "Google Ads Report"
    ws.append(fieldnames)
    for row in flattened:
        ws.append([row.get(name) for name in fieldnames])
    output = io.BytesIO()
    wb.save(output)
    return output.getvalue()


def _flatten(value: Any, prefix: str = "") -> dict[str, Any]:
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for key, item in value.items():
            path = f"{prefix}.{key}" if prefix else key
            out.update(_flatten(item, path))
        return out
    if isinstance(value, list):
        return {prefix: ", ".join(str(v) for v in value)}
    return {prefix: value}
