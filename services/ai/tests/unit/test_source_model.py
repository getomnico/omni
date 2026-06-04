from __future__ import annotations

import pytest

from db.models import Source

pytestmark = pytest.mark.unit


def test_source_from_row_accepts_flat_row():
    source = Source.from_row(
        {
            "id": "src-1",
            "name": "My Drive",
            "source_type": "google_drive",
            "is_active": True,
            "is_deleted": False,
        }
    )

    assert source.id == "src-1"
    assert source.name == "My Drive"
    assert source.source_type == "google_drive"
    assert source.is_active is True
    assert source.is_deleted is False


def test_source_from_row_accepts_connector_manager_overview():
    source = Source.from_row(
        {
            "source": {
                "id": "src-2",
                "name": "Team Slack",
                "source_type": "slack",
                "is_active": True,
                "is_deleted": False,
                "config": {},
            },
            "health": "healthy",
            "sync_runs": [],
        }
    )

    assert source.id == "src-2"
    assert source.name == "Team Slack"
    assert source.source_type == "slack"
    assert source.is_active is True
    assert source.is_deleted is False
