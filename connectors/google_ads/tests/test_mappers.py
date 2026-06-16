from google_ads_connector.mappers import (
    has_metric_keys,
    map_row_to_document,
    render_content,
    strip_metrics,
)


def test_mapper_excludes_numeric_metrics():
    row = {
        "campaign": {
            "id": "123",
            "name": "Brand Search",
            "resource_name": "customers/1/campaigns/123",
            "status": "ENABLED",
            "advertising_channel_type": "SEARCH",
        },
        "metrics": {
            "clicks": 42,
            "impressions": 100,
            "cost_micros": 123456,
            "conversions": 3,
        },
    }

    cleaned = strip_metrics(row)
    content = render_content("campaign", "1", row)
    doc = map_row_to_document(
        entity_type="campaign", customer_id="1", row=row, content_id="cid"
    )

    assert not has_metric_keys(cleaned)
    assert "metrics" not in doc.metadata.extra["google_ads"]["raw"]
    assert "clicks" not in content.lower()
    assert "cost_micros" not in content.lower()
    assert doc.external_id == "google_ads:1:campaign:123"
    assert doc.attributes["campaign_id"] == "123"
    assert doc.permissions.public is True


def test_standalone_structural_resource_mapping_attributes():
    row = {
        "shared_set": {
            "id": "456",
            "name": "Brand negatives",
            "resource_name": "customers/1/sharedSets/456",
            "type": "NEGATIVE_KEYWORDS",
            "status": "ENABLED",
        }
    }

    doc = map_row_to_document(
        entity_type="shared_set", customer_id="1", row=row, content_id="cid"
    )

    assert doc.metadata.content_type == "google_ads_shared_set"
    assert doc.attributes["shared_set_id"] == "456"
    assert doc.attributes["status"] == "ENABLED"
