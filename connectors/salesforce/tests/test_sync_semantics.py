"""Connector-level regression tests for checkpoint, deletion, and realtime semantics.

These tests drive ``SalesforceConnector.sync`` directly against the mock
Salesforce HTTP API with an in-memory fake SDK client, so they control the
checkpoint and ``is_resume`` inputs the connector-manager would provide.
"""

from __future__ import annotations

import asyncio
import logging
from datetime import UTC, datetime, timedelta
from typing import Any

import pytest
from omni_connector import SyncMode

import salesforce_connector.connector as connector_module
import salesforce_connector.pagination as pagination_module
from salesforce_connector.config import schema_fingerprint
from salesforce_connector.connector import SalesforceConnector
from salesforce_connector.models import SalesforceSourceConfig, group_email
from tests.conftest import (
    FakeSdkClient,
    MockSalesforceAPI,
    make_sync_context,
    salesforce_config,
)

CREDENTIALS_HEAD = {"access_token": "test-token"}


@pytest.fixture
def account_only() -> dict[str, object]:
    return {"enabled_objects": ["Account"]}


@pytest.fixture
def fast_pages(monkeypatch: pytest.MonkeyPatch) -> None:
    """Shrink pages so multi-page scans stay fast."""
    monkeypatch.setattr(connector_module, "PAGE_SIZE", 2)
    monkeypatch.setattr(connector_module, "CHECKPOINT_INTERVAL", 1)
    monkeypatch.setattr(pagination_module, "PAGE_SIZE", 2)


def _config(mock_server: str, **overrides: object) -> dict[str, object]:
    return salesforce_config(mock_server, **overrides)


def _event_type(event: Any) -> str:
    value = event.type
    return value.value if hasattr(value, "value") else str(value)


def _fingerprint(config: dict[str, object]) -> str:
    try:
        parsed = SalesforceSourceConfig.from_mapping(config)
    except ValueError:
        # Invalid configuration must fail during sync(), not here.
        return "invalid"
    return schema_fingerprint(
        parsed.enabled_objects,
        parsed.public_read_objects,
        sync_users=parsed.sync_users,
        sync_groups=parsed.sync_groups,
        sync_shares=parsed.sync_shares,
        grant_access_using_hierarchies=parsed.grant_access_using_hierarchies,
    )


# Emulates the manager's durable per-source connector_state across the
# sequential runs inside one test.
_CONNECTOR_STATES: dict[str, dict[str, object]] = {}


@pytest.fixture(autouse=True)
def _reset_connector_state_store() -> None:
    _CONNECTOR_STATES.clear()


async def run_connector(
    mock_server: str,
    *,
    checkpoint: dict[str, Any] | None,
    mode: SyncMode,
    is_resume: bool,
    sync_run_id: str = "run-1",
    config: dict[str, object] | None = None,
    cancel_after: int | None = None,
    cancel_on_sleep: bool = False,
    interrupt_before_propagation: bool = False,
) -> tuple[FakeSdkClient, Any, SalesforceConnector]:
    connector = SalesforceConnector()
    fake = FakeSdkClient()
    source_config = config or _config(mock_server)
    source_id = "src-1"
    stored = _CONNECTOR_STATES.setdefault(source_id, {})
    stored.setdefault("schema_fingerprint", _fingerprint(source_config))
    ctx = make_sync_context(
        fake,
        checkpoint,
        sync_mode=mode,
        is_resume=is_resume,
        sync_run_id=sync_run_id,
        connector_state=stored,
    )
    if cancel_after is not None:
        original = connector._emit_record
        counter = {"n": 0}

        async def cancelled_emit(**kwargs: Any) -> None:
            await original(**kwargs)
            counter["n"] += 1
            if counter["n"] >= cancel_after:
                ctx._set_cancelled()

        connector._emit_record = cancelled_emit  # type: ignore[method-assign]

    if cancel_on_sleep:

        async def stop_after_poll(*args: Any, **kwargs: Any) -> None:
            ctx._set_cancelled()

        connector._sleep_with_heartbeat = stop_after_poll  # type: ignore[method-assign]

    if interrupt_before_propagation:

        async def interrupted_emit_changed(**kwargs: Any) -> Any:
            ctx._set_cancelled()
            return kwargs["checkpoint"]

        connector._emit_changed_parents = interrupted_emit_changed  # type: ignore[method-assign]

    await connector.sync(
        source_config,
        {**CREDENTIALS_HEAD, "instance_url": mock_server},
        checkpoint,
        ctx,
    )
    _CONNECTOR_STATES[source_id] = dict(ctx.connector_state)
    return fake, ctx, connector


def _account_queries(mock: MockSalesforceAPI) -> list[str]:
    return [
        query
        for query in mock.queries
        if "FROM Account " in query or "FROM Account\n" in query
    ]


def _published(fake: FakeSdkClient) -> dict[str, Any]:
    assert fake.completed == 1, f"sync did not complete: failures={fake.failures}"
    return fake.checkpoints[-1]


async def _baseline_full(
    mock_salesforce_api: MockSalesforceAPI,
    mock_server: str,
    *,
    config: dict[str, object],
    accounts: int = 5,
    recent_modstamps: bool = False,
) -> tuple[FakeSdkClient, dict[str, Any]]:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    for i in range(1, accounts + 1):
        mock_salesforce_api.add_account(
            f"0010000000000{i:02d}",
            name=f"Acme {i}",
            **(
                {"system_modstamp": _modstamp()}
                if recent_modstamps
                else {}
            ),
        )
    fake, _, _ = await run_connector(
        mock_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    return fake, _published(fake)


# ---------------------------------------------------------------------------
# Fresh vs resume checkpoint state
# ---------------------------------------------------------------------------


async def test_fresh_full_ignores_previous_source_checkpoint(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )

    mock_salesforce_api.queries.clear()
    fake, ctx, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert ctx.documents_scanned == 5
    # A full scan must start at the beginning of the Id range, never from a
    # cursor stored by an earlier pass.
    assert any("ORDER BY Id" in query for query in _account_queries(mock_salesforce_api))
    assert all("Id >" not in query for query in _account_queries(mock_salesforce_api))


async def test_two_consecutive_full_syncs_scan_everything(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )
    fake, ctx, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert ctx.documents_scanned == 5
    # A full pass always emits creates (upserts), never incremental updates.
    assert set(fake.documents) == {f"Account:0010000000000{i:02d}" for i in range(1, 6)}
    assert fake.updated_ids == []


async def test_fresh_incremental_does_not_reuse_keyset_cursor(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )
    mock_salesforce_api.add_account("001000000000099", name="New", system_modstamp=_modstamp())

    fake1, ctx1, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert ctx1.documents_scanned == 1
    published2 = _published(fake1)

    mock_salesforce_api.queries.clear()
    _, ctx2, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published2,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-3",
        config=config,
    )
    # A reused keyset cursor would resume after the record scanned in run-2 and
    # skip it; the overlap window must re-cover it from the committed boundary.
    assert ctx2.documents_scanned == 1
    account_queries = _account_queries(mock_salesforce_api)
    assert account_queries
    # Every fresh delta starts from the committed watermark window; it must not
    # continue from a completed pass's keyset cursor.
    assert all("(SystemModstamp >" not in query for query in account_queries)
    assert all("SystemModstamp >=" in query for query in account_queries)


async def test_resume_without_run_checkpoint_uses_source_baseline(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )
    mock_salesforce_api.add_account("001000000000098", name="Newer", system_modstamp=_modstamp())
    mock_salesforce_api.queries.clear()

    fake, ctx, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-lost",
        config=config,
    )
    # The manager fell back to the published source checkpoint because the run
    # never saved one. It must be treated as committed state, not run progress.
    assert ctx.documents_scanned == 1
    account_queries = _account_queries(mock_salesforce_api)
    assert all("SystemModstamp >=" in query for query in account_queries)


async def test_same_run_resume_skips_completed_objects(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server, enabled_objects=["Account", "Contact"]
    )
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    # Touch the account and add a contact so the incremental pass has records.
    for record in mock_salesforce_api.objects["Account"]:
        record["SystemModstamp"] = _modstamp()
    mock_salesforce_api.add_contact(
        "003000000000099", first_name="New", last_name="Person", system_modstamp=_modstamp()
    )

    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        cancel_after=2,
    )
    run_checkpoint = fake1.checkpoints[-1]
    assert run_checkpoint["progress"] is not None
    assert "Account" in run_checkpoint["progress"]["records_completed"]
    assert "Contact" not in run_checkpoint["progress"]["records_completed"]

    mock_salesforce_api.queries.clear()
    _, ctx2, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=run_checkpoint,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-2",
        config=config,
    )
    # Account already completed in this run: it must not be re-queried.
    assert not _account_queries(mock_salesforce_api)
    assert ctx2.documents_scanned == 1


async def test_cancelled_run_progress_is_not_used_by_fresh_run(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )
    mock_salesforce_api.add_account("001000000000097", name="Late", system_modstamp=_modstamp())

    fake_cancelled, ctx_cancelled, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        cancel_after=1,
    )
    assert ctx_cancelled.is_cancelled()
    assert fake_cancelled.completed == 0

    # A fresh run starts from the published source checkpoint, not the
    # abandoned run's unpromoted progress.
    fake_fresh, ctx_fresh, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-3",
        config=config,
    )
    assert ctx_fresh.documents_scanned == 1
    assert "Account:001000000000097" in fake_fresh.updated_ids


async def test_resume_within_multi_page_scan(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    fast_pages: None,
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api,
        mock_salesforce_server,
        config=config,
        accounts=5,
        recent_modstamps=True,
    )

    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        cancel_after=1,
    )
    run_checkpoint = fake1.checkpoints[-1]
    assert run_checkpoint["progress"] is not None
    assert run_checkpoint["progress"]["current_object"] == "Account"
    assert run_checkpoint["progress"]["record_cursor"] is not None

    fake2, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=run_checkpoint,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-2",
        config=config,
    )
    # Every record is emitted exactly once across the interrupted attempt and
    # its resume: the cursor continues the bounded window without gaps or
    # duplicates.
    emitted = fake1.updated_ids + fake2.updated_ids
    assert len(emitted) == 5
    assert len(set(emitted)) == 5
    assert set(emitted) == {f"Account:0010000000000{i:02d}" for i in range(1, 6)}


# ---------------------------------------------------------------------------
# Deletion coverage
# ---------------------------------------------------------------------------


async def test_deletion_within_committed_window_emits_tombstone(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=2
    )
    mock_salesforce_api.mark_deleted("Account", "001000000000001")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert "Account:001000000000001" in fake.deleted_ids


async def test_deletion_retention_gap_fails_without_advancing_state(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=3
    )
    committed = datetime.fromisoformat(published["objects"]["Account"]["deletion_through"])
    # Retention now starts after the committed boundary (a gap). mark_deleted
    # removes the live record, so re-emitting live records can never stand in
    # for the missing tombstone.
    mock_salesforce_api.deletion_earliest_override = (committed + timedelta(hours=1)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    mock_salesforce_api.mark_deleted("Account", "001000000000002")

    fake, ctx, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    # The connector cannot produce a reliable tombstone for the gap, so it
    # fails the run and publishes nothing.
    assert ctx.is_cancelled() is False
    assert fake.completed == 0
    assert fake.failures
    assert "deletion retention" in fake.failures[0].lower()
    assert fake.deleted_ids == []
    last = fake.checkpoints[-1]
    assert last["objects"]["Account"]["deletion_through"] == published["objects"]["Account"][
        "deletion_through"
    ]
    assert last["objects"]["Account"]["watermark"] == published["objects"]["Account"][
        "watermark"
    ]


async def test_deletion_failure_does_not_advance_boundaries(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=3
    )
    old_watermark = published["objects"]["Account"]["watermark"]
    old_deletion = published["objects"]["Account"]["deletion_through"]

    mock_salesforce_api.add_account("001000000000096", name="Late", system_modstamp=_modstamp())
    mock_salesforce_api.fail_deleted_objects.add("Account")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert fake.failures
    last = fake.checkpoints[-1]
    assert last["objects"]["Account"]["watermark"] == old_watermark
    assert last["objects"]["Account"]["deletion_through"] == old_deletion


async def test_latest_date_covered_earlier_than_window_catches_up(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account("001000000000001")
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0)

    mock_salesforce_api.mark_deleted("Account", "001000000000001")
    # Salesforce has only covered up to a minute ago, before the deletion but
    # inside the committed overlap window.
    covered_dt = datetime.now(UTC) - timedelta(seconds=60)
    covered = covered_dt.strftime("%Y-%m-%dT%H:%M:%SZ")
    mock_salesforce_api.deletion_latest_override = covered

    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    # The tombstone is not reported yet, and the boundary advances only to the
    # provider-confirmed coverage.
    assert "Account:001000000000001" not in fake1.deleted_ids
    published2 = _published(fake1)
    assert published2["objects"]["Account"]["deletion_through"] == datetime.fromisoformat(
        covered.replace("Z", "+00:00")
    ).isoformat()

    mock_salesforce_api.deletion_latest_override = None
    fake2, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published2,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-3",
        config=config,
    )
    assert "Account:001000000000001" in fake2.deleted_ids


# ---------------------------------------------------------------------------
# Realtime
# ---------------------------------------------------------------------------


async def test_realtime_failed_object_retains_watermark(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server,
        enabled_objects=["Account", "Contact"],
        realtime_poll_seconds=10,
    )
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_contact()
    fake0_rt, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0_rt)

    mock_salesforce_api.fail_query_objects.add("Account")
    fake_rt, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.REALTIME,
        is_resume=False,
        sync_run_id="run-rt",
        config=config,
        cancel_on_sleep=True,
    )
    assert fake_rt.completed == 0
    last = fake_rt.checkpoints[-1]
    # The failed object keeps its committed watermark; other objects advance.
    assert (
        last["objects"]["Account"]["watermark"] == published["objects"]["Account"]["watermark"]
    )
    assert (
        last["objects"]["Contact"]["watermark"]
        != published["objects"]["Contact"]["watermark"]
    )


async def test_realtime_and_scheduled_slots_are_independent(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config
    )
    realtime_config = _config(
        mock_salesforce_server, enabled_objects=["Account"], realtime_poll_seconds=10
    )

    async def scheduled() -> tuple[FakeSdkClient, Any, SalesforceConnector]:
        return await run_connector(
            mock_salesforce_server,
            checkpoint=published,
            mode=SyncMode.INCREMENTAL,
            is_resume=False,
            sync_run_id="run-sched",
            config=config,
        )

    async def realtime() -> tuple[FakeSdkClient, Any, SalesforceConnector]:
        return await run_connector(
            mock_salesforce_server,
            checkpoint=published,
            mode=SyncMode.REALTIME,
            is_resume=False,
            sync_run_id="run-rt",
            config=realtime_config,
            cancel_on_sleep=True,
        )

    (scheduled_fake, _, _), (realtime_fake, _, _) = await asyncio.gather(
        scheduled(), realtime()
    )
    # The scheduled run completes and publishes committed-state only; the
    # long-lived realtime run never promotes its run-scoped checkpoint.
    assert scheduled_fake.completed == 1
    assert realtime_fake.completed == 0
    scheduled_published = _published(scheduled_fake)
    assert scheduled_published["progress"] is None
    assert scheduled_published["objects"]["Account"]["watermark"] is not None
    assert all(cp["progress"] is not None for cp in realtime_fake.checkpoints)


# ---------------------------------------------------------------------------
# Share and group reconciliation
# ---------------------------------------------------------------------------


def _document_event(fake: FakeSdkClient, document_id: str) -> Any:
    for event in reversed(fake.events):
        if event.document_id == document_id and event.permissions is not None:
            return event
    raise AssertionError(f"document event not found for {document_id}")


def _account_event(fake: FakeSdkClient) -> Any:
    return _document_event(fake, "Account:001000000000001")


async def test_share_addition_updates_unchanged_parent(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    # The account's own modstamp did not change, but the new share is applied.
    assert "Account:001000000000001" in fake.updated_ids
    assert "manager@example.com" in _account_event(fake).permissions.users


async def test_share_revocation_updates_unchanged_parent(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0)
    assert "manager@example.com" in _account_event(fake0).permissions.users

    mock_salesforce_api.objects["AccountShare"].clear()
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert "Account:001000000000001" in fake1.updated_ids
    assert "manager@example.com" not in _account_event(fake1).permissions.users


async def test_sync_shares_disabled_issues_no_share_queries(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server, enabled_objects=["Account"], sync_shares=False
    )
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert not any("FROM AccountShare" in query for query in mock_salesforce_api.queries)
    assert "manager@example.com" not in _account_event(fake).permissions.users


async def test_standard_share_access_level_field_is_queried(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account", "Case"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_case()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    mock_salesforce_api.add_share(
        "CaseShare", parent_id="500000000000001", user_or_group_id="005000000000003"
    )

    await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    share_queries = [q for q in mock_salesforce_api.queries if "Share" in q]
    assert any(
        "SELECT Id, AccountId, UserOrGroupId, AccountAccessLevel, RowCause FROM AccountShare"
        in q
        for q in share_queries
    )
    assert any(
        "SELECT Id, CaseId, UserOrGroupId, CaseAccessLevel, RowCause FROM CaseShare" in q
        for q in share_queries
    )


async def test_role_group_share_grants_role_members(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="00G000000000003"
    )

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert group_email("00G000000000003") in _account_event(fake).permissions.groups
    memberships = {
        event.group_email: set(event.member_emails)
        for event in fake.events
        if _event_type(event) == "group_membership_sync"
    }
    # The 00G role group resolves through RelatedId to the role and below.
    assert memberships[group_email("00G000000000003")] == {
        "owner@example.com",
        "agent@example.com",
    }


async def test_oversized_share_snapshot_triggers_periodic_reconciliation(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Force the share-snapshot bound so diff state cannot be persisted.
    monkeypatch.setattr(connector_module, "MAX_SHARE_SNAPSHOT_ENTRIES", 0)
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=2
    )
    assert published["share_snapshot"]["oversized"] is False

    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    # With no diff state the connector re-emits every record of the affected
    # object so the new share is applied, then marks the snapshot oversized.
    assert {"Account:001000000000001", "Account:001000000000002"} <= set(fake.updated_ids)
    assert "manager@example.com" in _account_event(fake).permissions.users
    assert _published(fake)["share_snapshot"]["oversized"] is True


async def test_removed_group_membership_is_revoked(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.objects["Group"] = [
        group
        for group in mock_salesforce_api.objects["Group"]
        if group.get("Id") != "00G000000000002"
    ]

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    revocation = [
        event
        for event in fake.events
        if _event_type(event) == "group_membership_sync"
        and event.group_email == group_email("00G000000000002")
    ]
    assert revocation
    assert revocation[-1].member_emails == []


# ---------------------------------------------------------------------------
# Field capability discovery
# ---------------------------------------------------------------------------


async def test_hidden_optional_field_is_omitted(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_fields.setdefault("Account", set()).add("Industry")
    mock_salesforce_api.queries.clear()

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    account_queries = _account_queries(mock_salesforce_api)
    assert account_queries and all("Industry" not in query for query in account_queries)
    event = _account_event(fake)
    assert "industry" not in event.attributes


async def test_unavailable_relationship_keeps_object_queryable(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Contact"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_contact()
    mock_salesforce_api.hidden_fields.setdefault("Contact", set()).add("Account")
    mock_salesforce_api.queries.clear()

    fake, ctx, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert ctx.documents_scanned == 1
    contact_queries = [q for q in mock_salesforce_api.queries if "FROM Contact" in q]
    assert contact_queries and all("Account.Name" not in query for query in contact_queries)
    assert "Contact:003000000000001" in fake.documents
    # The relationship traversal is dropped, but the base object and its other
    # fields are still indexed.
    attributes = _document_event(fake, "Contact:003000000000001").attributes
    assert "account_name" not in attributes
    assert attributes["account_id"] == "001000000000001"
    assert attributes["email"] == "john@example.com"


async def test_missing_system_modstamp_uses_full_only_fallback(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_fields.setdefault("Account", set()).add("SystemModstamp")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake)
    # No incremental coverage is claimed for an object without SystemModstamp.
    assert published["objects"]["Account"]["watermark"] is None

    mock_salesforce_api.queries.clear()
    _, ctx2, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert ctx2.documents_scanned == 1
    account_queries = _account_queries(mock_salesforce_api)
    assert account_queries and all("SystemModstamp" not in query for query in account_queries)


async def test_missing_is_active_does_not_delete_users(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    caplog: pytest.LogCaptureFixture,
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_fields.setdefault("User", set()).add("IsActive")

    with caplog.at_level(logging.WARNING, logger="salesforce_connector.connector"):
        fake, _, _ = await run_connector(
            mock_salesforce_server,
            checkpoint=None,
            mode=SyncMode.FULL,
            is_resume=False,
            sync_run_id="run-1",
            config=config,
        )
    # The unknown-lifecycle path was actually taken...
    assert any("IsActive is unavailable" in record.message for record in caplog.records)
    # ...and unknown users are kept (indexed), never treated as deleted.
    person_events = [e for e in fake.events if _event_type(e) == "person_sync"]
    deleted_events = [e for e in fake.events if _event_type(e) == "person_deleted"]
    assert len(person_events) == 3
    assert deleted_events == []


async def test_unknown_visibility_object_is_rejected(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server, enabled_objects=["Account"], public_read_objects=["Bogus"]
    )
    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert fake.failures
    assert "public_read_objects" in fake.failures[0]


async def test_group_sync_without_users_is_rejected(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server,
        enabled_objects=["Account"],
        sync_users=False,
        sync_groups=False,
        sync_shares=True,
    )
    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert fake.failures
    assert "sync_users" in fake.failures[0]


def _modstamp() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%S.000+0000")


# ---------------------------------------------------------------------------
# Fixed resume window
# ---------------------------------------------------------------------------


async def test_same_run_resume_keeps_original_window(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    fast_pages: None,
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api,
        mock_salesforce_server,
        config=config,
        accounts=2,
        recent_modstamps=True,
    )
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        cancel_after=1,
    )
    run_checkpoint = fake1.checkpoints[-1]
    assert run_checkpoint["progress"] is not None
    window_end = datetime.fromisoformat(run_checkpoint["progress"]["window_end"])

    # A record created after the pass window must not be pulled into the
    # resumed pass, and the pass must not advance past its original window.
    future_modstamp = (datetime.now(UTC) + timedelta(minutes=5)).strftime(
        "%Y-%m-%dT%H:%M:%S.000+0000"
    )
    mock_salesforce_api.add_account(
        "001000000000099", name="After window", system_modstamp=future_modstamp
    )
    fake2, ctx2, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=run_checkpoint,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-2",
        config=config,
    )
    assert "Account:001000000000099" not in fake2.updated_ids
    published2 = _published(fake2)
    assert (
        published2["objects"]["Account"]["watermark"]
        == (window_end - timedelta(seconds=900)).isoformat()
    )
    assert ctx2.is_cancelled() is False


# ---------------------------------------------------------------------------
# Share snapshot interruption / resume
# ---------------------------------------------------------------------------


async def test_share_addition_survives_interruption_and_resume(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )

    # Interrupt after the candidate snapshot is saved but before the affected
    # parent is emitted.
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        interrupt_before_propagation=True,
    )
    assert fake1.completed == 0
    run_checkpoint = fake1.checkpoints[-1]
    # Committed snapshot is untouched; the candidate is only pending.
    assert run_checkpoint["share_snapshot"]["grants"]["AccountShare"] == {}
    assert (
        run_checkpoint["progress"]["pending_share_snapshot"]["grants"]["AccountShare"]
        != {}
    )

    fake2, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=run_checkpoint,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-2",
        config=config,
    )
    assert "manager@example.com" in _account_event(fake2).permissions.users
    published2 = _published(fake2)
    assert "001000000000001" in published2["share_snapshot"]["grants"]["AccountShare"]
    assert published2["share_snapshot"]["oversized"] is False
    assert published2["progress"] is None


async def test_share_revocation_survives_interruption_and_resume(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0)
    assert "manager@example.com" in _account_event(fake0).permissions.users

    mock_salesforce_api.objects["AccountShare"].clear()
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
        interrupt_before_propagation=True,
    )
    assert fake1.completed == 0
    run_checkpoint = fake1.checkpoints[-1]
    assert run_checkpoint["share_snapshot"]["grants"]["AccountShare"] != {}

    fake2, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=run_checkpoint,
        mode=SyncMode.INCREMENTAL,
        is_resume=True,
        sync_run_id="run-2",
        config=config,
    )
    assert "manager@example.com" not in _account_event(fake2).permissions.users
    published2 = _published(fake2)
    assert published2["share_snapshot"]["grants"]["AccountShare"] == {}


# ---------------------------------------------------------------------------
# Unresolved share state
# ---------------------------------------------------------------------------


async def test_share_query_failure_fails_scheduled_sync(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.fail_query_objects.add("AccountShare")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert fake.completed == 0
    assert fake.failures
    assert "sharing" in fake.failures[0].lower()
    # No record is committed with an incomplete permission set.
    assert fake.updated_ids == []
    assert fake.documents == {}
    last = fake.checkpoints[-1]
    assert last["objects"]["Account"]["watermark"] == published["objects"]["Account"][
        "watermark"
    ]


async def test_share_describe_failure_fails_scheduled_sync(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.hidden_objects.add("AccountShare")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    assert fake.completed == 0
    assert fake.failures
    assert fake.documents == {}


async def test_share_query_failure_leaves_realtime_object_untouched(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server, enabled_objects=["Account"], realtime_poll_seconds=10
    )
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=2
    )
    mock_salesforce_api.fail_query_objects.add("AccountShare")

    fake_rt, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.REALTIME,
        is_resume=False,
        sync_run_id="run-rt",
        config=config,
        cancel_on_sleep=True,
    )
    assert fake_rt.completed == 0
    last = fake_rt.checkpoints[-1]
    # Sharing is unresolved, so the object keeps its committed state.
    assert (
        last["objects"]["Account"]["watermark"]
        == published["objects"]["Account"]["watermark"]
    )


# ---------------------------------------------------------------------------
# Permission-setting and capability fingerprints
# ---------------------------------------------------------------------------


async def test_sync_shares_disabled_forces_full_reexport(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config_on = _config(
        mock_salesforce_server, enabled_objects=["Account"], sync_shares=True
    )
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config_on,
    )
    published = _published(fake0)
    assert "manager@example.com" in _account_event(fake0).permissions.users

    config_off = _config(
        mock_salesforce_server, enabled_objects=["Account"], sync_shares=False
    )
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config_off,
    )
    # Disabling sharing must remove the stale grant from the unchanged record.
    assert "Account:001000000000001" in fake1.updated_ids
    assert "manager@example.com" not in _account_event(fake1).permissions.users


async def test_hierarchy_disable_forces_full_reexport(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config_on = _config(
        mock_salesforce_server,
        enabled_objects=["Account"],
        grant_access_using_hierarchies=True,
    )
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config_on,
    )
    published = _published(fake0)
    from salesforce_connector.models import direct_role_email

    assert direct_role_email("00E000000000003") in _account_event(fake0).permissions.groups

    config_off = _config(
        mock_salesforce_server,
        enabled_objects=["Account"],
        grant_access_using_hierarchies=False,
    )
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config_off,
    )
    assert "Account:001000000000001" in fake1.updated_ids
    assert (
        direct_role_email("00E000000000003")
        not in _account_event(fake1).permissions.groups
    )


async def test_field_unavailable_after_baseline_reexports_documents(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0)
    assert _account_event(fake0).attributes["industry"] == "Technology"

    mock_salesforce_api.hidden_fields.setdefault("Account", set()).add("Industry")
    fake1, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.INCREMENTAL,
        is_resume=False,
        sync_run_id="run-2",
        config=config,
    )
    # The record itself did not change, but field-level security did: it is
    # re-emitted so the stale attribute is removed.
    assert "Account:001000000000001" in fake1.updated_ids
    assert "industry" not in _account_event(fake1).attributes


# ---------------------------------------------------------------------------
# Inactive / unknown users
# ---------------------------------------------------------------------------


async def test_inactive_owner_is_not_granted(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_user(
        "005000000000009",
        email="inactive@example.com",
        name="Inactive Owner",
        is_active=False,
        role_id=None,
    )
    mock_salesforce_api.add_account(owner_id="005000000000009")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert "inactive@example.com" not in _account_event(fake).permissions.users
    assert _event_type(_document_event(fake, "Account:001000000000001")) == "document_created"


async def test_unknown_is_active_owner_is_not_granted(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.hidden_fields.setdefault("User", set()).add("IsActive")

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    # Unknown lifecycle never grants access...
    assert "owner@example.com" not in _account_event(fake).permissions.users
    # ...but it also must not be emitted as a deletion.
    assert [e for e in fake.events if _event_type(e) == "person_deleted"] == []
    assert len([e for e in fake.events if _event_type(e) == "person_sync"]) == 3


async def test_inactive_share_target_is_not_granted(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(mock_salesforce_server, enabled_objects=["Account"])
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_user(
        "005000000000009",
        email="inactive@example.com",
        name="Inactive Target",
        is_active=False,
        role_id=None,
    )
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000009"
    )

    fake, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    assert "inactive@example.com" not in _account_event(fake).permissions.users


# ---------------------------------------------------------------------------
# Realtime reconciliation and auth
# ---------------------------------------------------------------------------


async def test_realtime_preserves_reconciliation_for_failed_object(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config = _config(
        mock_salesforce_server,
        enabled_objects=["Account", "Contact"],
        realtime_poll_seconds=10,
    )
    mock_salesforce_api.reset()
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_contact()
    fake0, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=None,
        mode=SyncMode.FULL,
        is_resume=False,
        sync_run_id="run-1",
        config=config,
    )
    published = _published(fake0)

    monkeypatch.setattr(connector_module, "MAX_SHARE_SNAPSHOT_ENTRIES", 0)
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    mock_salesforce_api.fail_query_objects.add("Contact")

    fake_rt, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.REALTIME,
        is_resume=False,
        sync_run_id="run-rt",
        config=config,
        cancel_on_sleep=True,
    )
    assert fake_rt.completed == 0
    assert "Account:001000000000001" in fake_rt.updated_ids
    last = fake_rt.checkpoints[-1]
    # The failed object stays queued for reconciliation.
    assert last["progress"]["full_reconciliation"] == ["Contact"]


async def test_realtime_authentication_error_is_fatal(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    config = _config(
        mock_salesforce_server, enabled_objects=["Account"], realtime_poll_seconds=10
    )
    _, published = await _baseline_full(
        mock_salesforce_api, mock_salesforce_server, config=config, accounts=1
    )
    mock_salesforce_api.fail_auth_objects.add("Account")

    fake_rt, _, _ = await run_connector(
        mock_salesforce_server,
        checkpoint=published,
        mode=SyncMode.REALTIME,
        is_resume=False,
        sync_run_id="run-rt",
        config=config,
        cancel_on_sleep=True,
    )
    # Auth failures are run-fatal and must not be swallowed per object.
    assert fake_rt.failures
    assert "Authentication" in fake_rt.failures[0]
    assert fake_rt.completed == 0
