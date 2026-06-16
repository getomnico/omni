"""Map Google Ads rows to Omni Documents."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

METRIC_KEYS = {
    "metrics",
    "clicks",
    "impressions",
    "cost_micros",
    "cost",
    "ctr",
    "average_cpc",
    "conversions",
    "conversions_value",
    "roas",
    "all_conversions",
}

CONTENT_TYPES = {
    "customer": "google_ads_customer",
    "campaign": "google_ads_campaign",
    "campaign_budget": "google_ads_campaign_budget",
    "bidding_strategy": "google_ads_bidding_strategy",
    "ad_group": "google_ads_ad_group",
    "ad_group_ad": "google_ads_ad",
    "asset": "google_ads_asset",
    "keyword_view": "google_ads_criterion",
    "shared_set": "google_ads_shared_set",
    "user_list": "google_ads_audience",
    "conversion_action": "google_ads_conversion_goal",
    "recommendation": "google_ads_recommendation",
}


def value_at(row: dict[str, Any], path: str) -> Any:
    current: Any = row
    for part in path.split("."):
        if isinstance(current, dict):
            current = current.get(part) or current.get(_to_camel(part))
        else:
            current = getattr(current, part, None)
        if current is None:
            return None
    return _normalize_value(current)


def _to_camel(value: str) -> str:
    pieces = value.split("_")
    return pieces[0] + "".join(p.capitalize() for p in pieces[1:])


def _normalize_value(value: Any) -> Any:
    if hasattr(value, "name") and not isinstance(value, str):
        return value.name
    if isinstance(value, list):
        return [_normalize_value(v) for v in value]
    if isinstance(value, dict):
        return {
            k: _normalize_value(v) for k, v in value.items() if not _is_metric_key(k)
        }
    return value


def _is_metric_key(key: str) -> bool:
    normalized = key.lower()
    return normalized in METRIC_KEYS or normalized.startswith("metrics.")


def strip_metrics(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            k: strip_metrics(v)
            for k, v in value.items()
            if not _is_metric_key(k) and k.lower() != "metrics"
        }
    if isinstance(value, list):
        return [strip_metrics(v) for v in value]
    return _normalize_value(value)


def has_metric_keys(value: Any) -> bool:
    if isinstance(value, dict):
        return any(_is_metric_key(k) or has_metric_keys(v) for k, v in value.items())
    if isinstance(value, list):
        return any(has_metric_keys(v) for v in value)
    return False


def resource_id(resource_name: str | None) -> str:
    if not resource_name:
        return "unknown"
    return resource_name.split("/")[-1]


def google_ads_url(
    customer_id: str, entity_type: str, entity_id: str | None = None
) -> str:
    base = f"https://ads.google.com/aw/overview?ocid={customer_id}"
    if entity_type == "campaign" and entity_id:
        return f"https://ads.google.com/aw/campaigns?campaignId={entity_id}&ocid={customer_id}"
    return base


def _title(entity_type: str, row: dict[str, Any]) -> str:
    candidates = [
        f"{entity_type}.name",
        f"{entity_type}.descriptive_name",
        "customer.descriptive_name",
        "ad_group.name",
        "campaign.name",
        "campaign_budget.name",
        "asset.name",
        "user_list.name",
        "conversion_action.name",
        "recommendation.type",
    ]
    for path in candidates:
        value = value_at(row, path)
        if value:
            return str(value)
    resource = value_at(row, f"{entity_type}.resource_name") or value_at(
        row, "resource_name"
    )
    display_type = entity_type.replace("_", " ").title()
    display_id = resource_id(str(resource) if resource else None)
    return f"Google Ads {display_type} {display_id}"


def _entity_id(entity_type: str, row: dict[str, Any]) -> str:
    direct = value_at(row, f"{entity_type}.id")
    if direct is not None:
        return str(direct)
    resource = value_at(row, f"{entity_type}.resource_name") or value_at(
        row, "resource_name"
    )
    return resource_id(str(resource) if resource else None)


def _status(entity_type: str, row: dict[str, Any]) -> str | None:
    value = value_at(row, f"{entity_type}.status")
    if value is None and entity_type == "ad_group_ad":
        value = value_at(row, "ad_group_ad.status")
    return str(value) if value is not None else None


def render_content(entity_type: str, customer_id: str, row: dict[str, Any]) -> str:
    cleaned = strip_metrics(row)
    lines = [
        f"Google Ads {entity_type.replace('_', ' ').title()}",
        f"Customer ID: {customer_id}",
        f"Title: {_title(entity_type, row)}",
    ]
    for key, value in _flatten(cleaned).items():
        if value not in (None, "", [], {}):
            lines.append(f"{key}: {value}")
    return "\n".join(lines)


def _flatten(value: Any, prefix: str = "") -> dict[str, Any]:
    if not isinstance(value, dict):
        return {prefix: value} if prefix else {}
    out: dict[str, Any] = {}
    for key, item in value.items():
        path = f"{prefix}.{key}" if prefix else key
        if isinstance(item, dict):
            out.update(_flatten(item, path))
        else:
            out[path] = item
    return out


def attributes_for(
    entity_type: str, customer_id: str, row: dict[str, Any]
) -> dict[str, Any]:
    campaign_id = value_at(row, "campaign.id") or _id_from_resource(
        value_at(row, "campaign.resource_name")
    )
    ad_group_id = value_at(row, "ad_group.id") or _id_from_resource(
        value_at(row, "ad_group.resource_name")
    )
    labels = value_at(row, "campaign.labels") or value_at(row, "ad_group.labels") or []
    attrs: dict[str, Any] = {
        "source_type": "google_ads",
        "entity_type": entity_type,
        "customer_id": customer_id,
        "resource_name": value_at(row, f"{entity_type}.resource_name")
        or value_at(row, "resource_name"),
        "status": _status(entity_type, row),
        "channel_type": value_at(row, "campaign.advertising_channel_type"),
        "campaign_id": str(campaign_id) if campaign_id is not None else None,
        "ad_group_id": str(ad_group_id) if ad_group_id is not None else None,
        "asset_id": _first_present(
            value_at(row, "asset.id"),
            _id_from_resource(value_at(row, "campaign_asset.asset")),
            _id_from_resource(value_at(row, "ad_group_asset.asset")),
        ),
        "criterion_id": _first_present(
            value_at(row, "ad_group_criterion.criterion_id"),
            value_at(row, "campaign_criterion.criterion_id"),
            value_at(row, "shared_criterion.criterion_id"),
        ),
        "criterion_type": _first_present(
            value_at(row, "ad_group_criterion.type"),
            value_at(row, "campaign_criterion.type"),
            value_at(row, "shared_criterion.type"),
        ),
        "shared_set_id": _first_present(
            value_at(row, "shared_set.id"),
            _id_from_resource(value_at(row, "shared_criterion.shared_set")),
        ),
        "bidding_strategy_type": value_at(row, "bidding_strategy.type"),
        "labels": labels,
    }
    return {k: v for k, v in attrs.items() if v not in (None, "", [])}


def _id_from_resource(value: Any) -> str | None:
    if not value:
        return None
    return str(value).split("/")[-1]


def _first_present(*values: Any) -> str | None:
    for value in values:
        if value not in (None, "", [], {}):
            return str(value)
    return None


def map_row_to_document(
    *,
    entity_type: str,
    customer_id: str,
    row: dict[str, Any],
    content_id: str,
) -> Document:
    clean_row = strip_metrics(row)
    entity_id = _entity_id(entity_type, row)
    resource = value_at(row, f"{entity_type}.resource_name") or value_at(
        row, "resource_name"
    )
    extra = {
        "google_ads": {
            "customer_id": customer_id,
            "entity_type": entity_type,
            "entity_id": entity_id,
            "resource_name": resource,
            "raw": clean_row,
        }
    }
    updated_at = datetime.now().astimezone()
    return Document(
        external_id=f"google_ads:{customer_id}:{entity_type}:{entity_id}",
        title=_title(entity_type, row),
        content_id=content_id,
        metadata=DocumentMetadata(
            title=_title(entity_type, row),
            updated_at=updated_at,
            content_type=CONTENT_TYPES.get(entity_type, f"google_ads_{entity_type}"),
            mime_type="application/json",
            url=google_ads_url(customer_id, entity_type, entity_id),
            extra=extra,
        ),
        permissions=DocumentPermissions(public=True, users=[], groups=[]),
        attributes=attributes_for(entity_type, customer_id, row),
    )
