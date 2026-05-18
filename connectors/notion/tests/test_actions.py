"""Tests for Notion connector actions."""

import csv
import io

import pytest

from notion_connector import NotionConnector

from .conftest import _block_payload

DS_ID = "ds-00000000-0000-0000-0000-000000000abc"
ROW_ONE_ID = "pg-00000000-0000-0000-0000-000000000101"
ROW_TWO_ID = "pg-00000000-0000-0000-0000-000000000102"


def test_export_data_source_csv_schema_avoids_top_level_composition_keywords():
    action = NotionConnector().actions[0]

    assert action.name == "export_data_source_csv"
    assert not {"anyOf", "oneOf", "allOf"} & set(action.input_schema)


@pytest.mark.asyncio
async def test_export_data_source_csv_includes_omni_external_id(
    mock_notion_api,
    mock_notion_server,
):
    mock_notion_api.reset()
    mock_notion_api.add_data_source(
        DS_ID,
        "Projects",
        {
            "Name": {"id": "title", "name": "Name", "type": "title", "title": {}},
            "Status": {
                "id": "status",
                "name": "Status",
                "type": "select",
                "select": {},
            },
            "Notes": {
                "id": "notes",
                "name": "Notes",
                "type": "rich_text",
                "rich_text": {},
            },
        },
    )
    mock_notion_api.add_data_source_entry(
        DS_ID,
        ROW_ONE_ID,
        "Alpha",
        {
            "Name": {
                "id": "title",
                "type": "title",
                "title": [
                    {
                        "type": "text",
                        "text": {"content": "Alpha"},
                        "plain_text": "Alpha",
                    }
                ],
            },
            "Status": {
                "id": "status",
                "type": "select",
                "select": {"name": "In Progress", "color": "blue"},
            },
            "Notes": {
                "id": "notes",
                "type": "rich_text",
                "rich_text": [
                    {
                        "type": "text",
                        "text": {"content": 'Has comma, quote ", and newline\nhere'},
                        "plain_text": 'Has comma, quote ", and newline\nhere',
                    }
                ],
            },
        },
        [_block_payload("blk-row-1", "paragraph", "Alpha body")],
    )
    mock_notion_api.add_data_source_entry(
        DS_ID,
        ROW_TWO_ID,
        "Beta",
        {
            "Name": {
                "id": "title",
                "type": "title",
                "title": [
                    {
                        "type": "text",
                        "text": {"content": "Beta"},
                        "plain_text": "Beta",
                    }
                ],
            },
            "Status": {
                "id": "status",
                "type": "select",
                "select": {"name": "Done", "color": "green"},
            },
        },
        [_block_payload("blk-row-2", "paragraph", "Beta body")],
    )

    response = await NotionConnector().execute_action(
        "export_data_source_csv",
        {
            "data_source_id": DS_ID,
            "include_content": True,
            "api_url": mock_notion_server,
        },
        {"token": "test-token"},
    )

    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/csv")
    assert response.headers["x-file-name"] == f"notion-data-source-{DS_ID[:8]}.csv"

    rows = list(csv.DictReader(io.StringIO(response.body.decode())))
    assert len(rows) == 2
    assert rows[0]["omni_external_id"] == f"notion:page:{ROW_ONE_ID}"
    assert rows[0]["notion_page_id"] == ROW_ONE_ID
    assert rows[0]["Name"] == "Alpha"
    assert rows[0]["Status"] == "In Progress"
    assert rows[0]["Notes"] == 'Has comma, quote ", and newline\nhere'
    assert rows[0]["notion_content"] == "Alpha body"
    assert rows[1]["omni_external_id"] == f"notion:page:{ROW_TWO_ID}"
    assert rows[1]["Name"] == "Beta"
    assert rows[1]["Status"] == "Done"


@pytest.mark.asyncio
async def test_export_data_source_csv_accepts_service_credential_envelope(
    mock_notion_api,
    mock_notion_server,
):
    mock_notion_api.reset()
    mock_notion_api.add_data_source(
        DS_ID,
        "Projects",
        {"Name": {"id": "title", "name": "Name", "type": "title", "title": {}}},
    )
    mock_notion_api.add_data_source_entry(
        DS_ID,
        ROW_ONE_ID,
        "Alpha",
        {
            "Name": {
                "id": "title",
                "type": "title",
                "title": [
                    {
                        "type": "text",
                        "text": {"content": "Alpha"},
                        "plain_text": "Alpha",
                    }
                ],
            },
        },
        [],
    )

    response = await NotionConnector().execute_action(
        "export_data_source_csv",
        {"data_source_id": DS_ID, "api_url": mock_notion_server},
        {
            "id": "cred-1",
            "source_id": "src-1",
            "provider": "notion",
            "auth_type": "api_key",
            "credentials": {"token": "test-token"},
        },
    )

    assert response.status_code == 200
    rows = list(csv.DictReader(io.StringIO(response.body.decode())))
    assert rows[0]["omni_external_id"] == f"notion:page:{ROW_ONE_ID}"
