from __future__ import annotations

import pytest

from db.tool_approvals import ToolApproval, ToolApprovalStatus, ToolApprovalType
from routers.chat import (
    _inaccessible_intervention_ids,
    _intervention_ids_to_expire,
)

pytestmark = pytest.mark.unit


def _approval(approval_id: str, source_id: str | None) -> ToolApproval:
    return ToolApproval(
        id=approval_id,
        chat_id="chat-1",
        user_id="user-1",
        tool_name="google_drive__read",
        tool_input={},
        status=ToolApprovalStatus.PENDING,
        created_at=None,
        resolved_at=None,
        resolved_by=None,
        approval_type=ToolApprovalType.OAUTH,
        tool_call_id=f"call-{approval_id}",
        source_id=source_id,
        source_type="google_drive" if source_id else None,
        provider="google" if source_id else None,
        oauth_start_url="/api/oauth/start" if source_id else None,
    )


def test_foreign_source_interventions_are_not_resumable():
    interventions = [
        _approval("org", "org-source"),
        _approval("own", "own-source"),
        _approval("foreign", "foreign-source"),
        _approval("no-source", None),
    ]

    assert _inaccessible_intervention_ids(
        interventions,
        {"org-source", "own-source"},
        sources_fetch_succeeded=True,
    ) == {"foreign"}


def test_source_fetch_failure_hides_but_does_not_authorize_expiration():
    interventions = [
        _approval("legitimate", "source-that-may-still-exist"),
        _approval("no-source", None),
    ]

    # A failed fetch must fail closed for this response, while the caller's
    # sources_fetch_succeeded guard prevents destructive status updates.
    assert _inaccessible_intervention_ids(
        interventions, set(), sources_fetch_succeeded=False
    ) == {"legitimate"}
    assert _intervention_ids_to_expire(
        interventions, set(), sources_fetch_succeeded=False
    ) == set()
    assert _intervention_ids_to_expire(
        interventions, set(), sources_fetch_succeeded=True
    ) == {"legitimate"}
