from __future__ import annotations

import pytest

from db.models import Source, filter_sources_for_user

pytestmark = pytest.mark.unit


def test_source_from_row_accepts_flat_row():
    source = Source.from_row(
        {
            "id": "src-1",
            "name": "My Drive",
            "source_type": "google_drive",
            "is_active": True,
            "is_deleted": False,
            "scope": "user",
            "created_by": "user-1",
        }
    )

    assert source.id == "src-1"
    assert source.name == "My Drive"
    assert source.source_type == "google_drive"
    assert source.is_active is True
    assert source.is_deleted is False
    assert source.scope == "user"
    assert source.created_by == "user-1"


def test_filter_sources_for_user_keeps_org_and_own_personal_sources_only():
    sources = [
        Source("org", "Org", "drive", True, False, scope="org", created_by="admin"),
        Source("own", "Own", "drive", True, False, scope="user", created_by="user-1"),
        Source(
            "foreign", "Foreign", "drive", True, False, scope="user", created_by="user-2"
        ),
    ]

    assert [source.id for source in filter_sources_for_user(sources, "user-1")] == [
        "org",
        "own",
    ]
    assert [source.id for source in filter_sources_for_user(sources, None)] == ["org"]
