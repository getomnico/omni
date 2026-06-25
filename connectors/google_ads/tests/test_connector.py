import pytest
from omni_connector import SyncMode

from google_ads_connector.connector import (
    MAX_XLSX_ROWS,
    GoogleAdsConnector,
    _action_query_params,
    build_report_query,
    rows_to_csv,
    validate_gaql_for_action,
)


class FakeStorage:
    def __init__(self):
        self.saved = []

    async def save(self, content, content_type):
        self.saved.append((content, content_type))
        return f"content-{len(self.saved)}"


class FakeContext:
    def __init__(self, sync_mode=SyncMode.FULL, is_resume=False):
        self.sync_mode = sync_mode
        self.is_resume = is_resume
        self.content_storage = FakeStorage()
        self.docs = []
        self.errors = []
        self.checkpoints = []
        self.connector_states = []
        self.completed = None
        self.failed = None
        self.documents_scanned = 0
        self.documents_emitted = 0

    def is_cancelled(self):
        return False

    async def increment_scanned(self):
        self.documents_scanned += 1

    async def emit(self, doc):
        self.docs.append(doc)
        self.documents_emitted += 1

    async def emit_error(self, external_id, error):
        self.errors.append((external_id, error))

    async def save_checkpoint(self, checkpoint):
        self.checkpoints.append(checkpoint)

    async def save_connector_state(self, connector_state):
        self.connector_states.append(connector_state)

    async def complete(self, checkpoint=None):
        self.completed = checkpoint or {}

    async def fail(self, error):
        self.failed = error


class CancelAfterFirstScannedContext(FakeContext):
    def is_cancelled(self):
        return self.documents_scanned >= 1


@pytest.mark.asyncio
async def test_full_sync_with_mock_data():
    connector = GoogleAdsConnector()
    ctx = FakeContext()
    source_config = {
        "customer_ids": ["1"],
        "entity_types": ["campaign"],
        "mock_data": {
            "customers": {
                "1": {
                    "campaign": [
                        {
                            "campaign": {
                                "id": "123",
                                "name": "Brand",
                                "resource_name": "customers/1/campaigns/123",
                                "status": "ENABLED",
                            },
                            "metrics": {"clicks": 5},
                        }
                    ]
                }
            }
        },
    }
    credentials = {"developer_token": "dev", "access_token": "access"}

    await connector.sync(source_config, credentials, None, ctx)

    assert ctx.failed is None
    assert len(ctx.docs) == 1
    assert ctx.docs[0].external_id == "google_ads:1:campaign:123"
    assert "metrics" not in ctx.docs[0].metadata.extra["google_ads"]["raw"]
    assert ctx.checkpoints
    assert ctx.completed["schema_version"] == 1
    assert ctx.completed["last_successful_sync_at"]
    assert ctx.completed["last_completed_unit"] is None
    assert ctx.connector_states == []


@pytest.mark.asyncio
async def test_full_sync_cancellation_does_not_checkpoint_partial_entity():
    connector = GoogleAdsConnector()
    ctx = CancelAfterFirstScannedContext()
    source_config = {
        "customer_ids": ["1"],
        "entity_types": ["campaign"],
        "mock_data": {
            "customers": {
                "1": {
                    "campaign": [
                        {
                            "campaign": {
                                "id": "123",
                                "name": "Brand",
                                "resource_name": "customers/1/campaigns/123",
                                "status": "ENABLED",
                            }
                        },
                        {
                            "campaign": {
                                "id": "456",
                                "name": "Generic",
                                "resource_name": "customers/1/campaigns/456",
                                "status": "ENABLED",
                            }
                        },
                    ]
                }
            }
        },
    }

    await connector.sync(
        source_config,
        {"developer_token": "dev", "access_token": "access"},
        None,
        ctx,
    )

    assert ctx.failed == "Cancelled by user"
    assert len(ctx.docs) == 1
    assert ctx.checkpoints == []
    assert ctx.completed is None


@pytest.mark.asyncio
async def test_incremental_sync_uses_change_status_and_refetches():
    connector = GoogleAdsConnector()
    ctx = FakeContext(sync_mode=SyncMode.INCREMENTAL)
    source_config = {
        "customer_ids": ["1"],
        "entity_types": ["campaign"],
        "mock_data": {
            "customers": {
                "1": {
                    "change_status": [{"change_status": {"resource_type": "campaign"}}],
                    "campaign": [
                        {
                            "campaign": {
                                "id": "123",
                                "name": "Brand",
                                "resource_name": "customers/1/campaigns/123",
                                "status": "ENABLED",
                            }
                        }
                    ],
                }
            }
        },
    }

    await connector.sync(
        source_config,
        {"developer_token": "dev", "access_token": "access"},
        {
            "schema_version": 1,
            "last_successful_sync_at": "2025-01-01T00:00:00Z",
            "last_completed_unit": None,
        },
        ctx,
    )

    assert ctx.failed is None
    assert len(ctx.docs) == 1
    assert ctx.completed["schema_version"] == 1
    assert ctx.completed["last_successful_sync_at"]
    assert ctx.completed["last_completed_unit"] is None
    assert ctx.connector_states == []


def test_manifest_fields_and_oauth_config():
    connector = GoogleAdsConnector()
    oauth = connector.oauth_config()

    assert connector.name == "google_ads"
    assert connector.display_name == "Google Ads"
    assert connector.source_types == ["google_ads"]
    assert connector.sync_modes == ["full", "incremental"]
    assert connector.skills[0].id == "google_ads"
    assert "Google Ads Skill" in connector.skills[0].content
    assert oauth.provider == "google_ads"
    assert "https://www.googleapis.com/auth/adwords" in oauth.scopes["google_ads"].read


def test_gaql_validation_and_csv_export():
    assert validate_gaql_for_action("SELECT campaign.id FROM campaign") is None
    assert validate_gaql_for_action("DELETE FROM campaign")
    csv_text = rows_to_csv(
        [
            {
                "campaign": {"id": "1"},
                "metrics": {
                    "clicks": 2,
                    "cost_micros": 3_000_000,
                    "conversions": 1,
                    "conversions_value": 9,
                },
            }
        ],
        metadata={"report_type": "custom_gaql", "row_count": 1},
    )
    assert "# report_type: custom_gaql" in csv_text
    assert "# Schema" in csv_text
    assert "campaign.id" in csv_text
    assert "metrics.clicks" in csv_text
    assert "derived.cost" in csv_text
    assert "derived.roas" in csv_text


def test_action_query_params_clamps_export_limits():
    _, _, limit = _action_query_params(
        {
            "customer_id": "123",
            "query": "SELECT campaign.id FROM campaign",
            "limit": 999999,
        },
        max_limit=MAX_XLSX_ROWS,
    )

    assert limit == MAX_XLSX_ROWS


def test_curated_report_query_uses_date_range_and_metrics():
    query = build_report_query("campaign_performance", {"date_range": "LAST_7_DAYS"})

    assert "FROM campaign" in query
    assert "segments.date DURING LAST_7_DAYS" in query
    assert "metrics.cost_micros" in query
