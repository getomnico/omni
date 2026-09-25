from hubspot_connector import HubSpotConnector


def test_display_name_uses_brand_capitalization() -> None:
    assert HubSpotConnector().display_name == "HubSpot"
