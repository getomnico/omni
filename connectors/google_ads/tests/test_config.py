from google_ads_connector.config import GoogleAdsCredentials, GoogleAdsSourceConfig


def test_source_config_parses_customer_ids_and_entities():
    cfg = GoogleAdsSourceConfig.parse(
        {
            "customer_ids": "123-456-7890, 2223334444",
            "login_customer_id": "111-222-3333",
            "entity_types": ["campaign", "ad_group", "metrics_not_allowed"],
        },
        {},
    )

    assert cfg.customer_ids == ["1234567890", "2223334444"]
    assert cfg.login_customer_id == "1112223333"
    assert cfg.entity_types == ["campaign", "ad_group"]


def test_default_entity_types_include_standalone_structural_resources():
    cfg = GoogleAdsSourceConfig.parse({"customer_ids": ["123"]}, {})

    assert "bidding_strategy" in cfg.entity_types
    assert "shared_set" in cfg.entity_types
    assert "label" not in cfg.entity_types
    assert "customer_client" not in cfg.entity_types
    assert "campaign_asset" not in cfg.entity_types
    assert "ad_group_criterion" not in cfg.entity_types
    assert "customer_conversion_goal" not in cfg.entity_types


def test_credentials_parse_service_credential_envelope():
    creds = GoogleAdsCredentials.parse(
        {
            "credentials": {"access_token": "access", "refresh_token": "refresh"},
            "config": {
                "developer_token": "dev",
                "client_id": "client",
                "client_secret": "secret",
                "login_customer_id": "111-222-3333",
            },
        }
    )

    assert creds.developer_token == "dev"
    assert creds.access_token == "access"
    assert creds.refresh_token == "refresh"
    assert creds.login_customer_id == "1112223333"
    assert creds.to_google_ads_dict()["developer_token"] == "dev"
