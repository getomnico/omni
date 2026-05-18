"""Unit tests for Notion document mapping."""

from notion_connector.mappers import (
    extract_data_source_property_schema,
    extract_page_property_attributes,
    map_page_to_document,
)

from .conftest import _page_payload, _rich_text


def test_page_mapping_includes_structured_notion_attributes():
    page = _page_payload(
        "pg-structured",
        "Launch plan",
        parent={"type": "data_source_id", "data_source_id": "ds-structured"},
        properties={
            "Status": {
                "id": "status",
                "type": "status",
                "status": {"name": "In Progress", "color": "blue"},
            },
            "Priority": {
                "id": "priority",
                "type": "select",
                "select": {"name": "High", "color": "red"},
            },
            "Tags": {
                "id": "tags",
                "type": "multi_select",
                "multi_select": [{"name": "Search"}, {"name": "Backend"}],
            },
            "Estimate": {"id": "estimate", "type": "number", "number": 3.5},
            "Done": {"id": "done", "type": "checkbox", "checkbox": False},
            "Due Date": {
                "id": "due",
                "type": "date",
                "date": {"start": "2026-05-18", "end": "2026-05-22"},
            },
            "Owner": {
                "id": "owner",
                "type": "people",
                "people": [
                    {
                        "id": "user-123",
                        "name": "Praveen",
                        "person": {"email": "praveen@example.com"},
                    }
                ],
            },
            "Related": {
                "id": "related",
                "type": "relation",
                "relation": [{"id": "pg-related"}],
            },
            "Score": {
                "id": "score",
                "type": "formula",
                "formula": {"type": "number", "number": 42},
            },
            "Rollup": {
                "id": "rollup",
                "type": "rollup",
                "rollup": {
                    "type": "array",
                    "array": [
                        {"type": "rich_text", "rich_text": _rich_text("alpha")},
                        {"type": "number", "number": 7},
                    ],
                },
            },
        },
    )

    doc = map_page_to_document(
        page,
        content_id="content-1",
        permission_group="notion:workspace:test",
        is_data_source_entry=True,
    )

    assert doc.external_id == "notion:page:pg-structured"
    assert doc.metadata is not None
    assert doc.metadata.mime_type == "text/markdown"
    assert doc.attributes is not None
    assert doc.attributes["notion_object_type"] == "data_source_entry"
    assert doc.attributes["notion_data_source_id"] == "ds-structured"
    assert doc.attributes["notion_external_id"] == "notion:page:pg-structured"
    assert doc.attributes["notion_prop_status"] == "In Progress"
    assert doc.attributes["notion_prop_priority"] == "High"
    assert doc.attributes["notion_prop_tags"] == ["Search", "Backend"]
    assert doc.attributes["notion_prop_estimate"] == 3.5
    assert doc.attributes["notion_prop_done"] is False
    assert doc.attributes["notion_prop_due_date"] == "2026-05-18"
    assert doc.attributes["notion_prop_due_date_end"] == "2026-05-22"
    assert doc.attributes["notion_prop_owner"] == ["praveen@example.com"]
    assert doc.attributes["notion_prop_related"] == ["pg-related"]
    assert doc.attributes["notion_prop_score"] == 42
    assert doc.attributes["notion_prop_rollup"] == ["alpha", 7]


def test_property_attribute_slugs_are_stable_and_collision_safe():
    attrs = extract_page_property_attributes(
        {
            "Due Date": {
                "id": "one",
                "type": "date",
                "date": {"start": "2026-05-18"},
            },
            "Due-Date": {
                "id": "two",
                "type": "date",
                "date": {"start": "2026-05-19"},
            },
        }
    )

    assert attrs["notion_prop_due_date"] == "2026-05-18"
    assert attrs["notion_prop_due_date_2"] == "2026-05-19"
    assert attrs["notion_property_names"]["due_date"] == "Due Date"
    assert attrs["notion_property_names"]["due_date_2"] == "Due-Date"


def test_data_source_schema_uses_normalized_property_keys():
    schema = extract_data_source_property_schema(
        {
            "Name": {"type": "title"},
            "Due Date": {"type": "date"},
        }
    )

    assert schema == {
        "name": {"name": "Name", "type": "title"},
        "due_date": {"name": "Due Date", "type": "date"},
    }
