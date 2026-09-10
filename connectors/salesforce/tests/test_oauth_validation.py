"""Tests for OAuth credential validation and the source-binding contract."""

from __future__ import annotations

from datetime import UTC, datetime

import pytest
from omni_connector import OAuthCredentialFlow
from omni_connector.models import Source

from salesforce_connector.connector import SalesforceConnector


def _source(config: dict[str, object]) -> Source:
    return Source(
        id="src-1",
        name="Salesforce",
        source_type="salesforce",
        config=config,
        is_active=True,
        is_deleted=False,
        scope="org",
        created_at=datetime.now(UTC),
        updated_at=datetime.now(UTC),
        created_by="admin-1",
    )


def _binding_dict(binding: object) -> dict[str, str]:
    """Normalize a source binding for either SDK representation.

    Newer SDKs return a pydantic object (serialized with ``model_dump``) and
    older ones return a plain mapping. The web layer stores the serialized
    field bag, so assert on that shape.
    """
    if binding is None:
        return {}
    model_dump = getattr(binding, "model_dump", None)
    if callable(model_dump):
        dumped = model_dump(exclude_none=True)
        return {str(key): str(value) for key, value in dumped.items()}
    assert isinstance(binding, dict)
    return {str(key): str(value) for key, value in binding.items()}


def test_source_binding_prefers_reserved_key_over_legacy_keys() -> None:
    config = {
        "source_binding": {"organization_id": "00D000000000001"},
        "organization_id": "00Dstale",
        "instance_url": "https://legacy.my.salesforce.com",
    }
    assert SalesforceConnector._source_binding(config) == {
        "organization_id": "00D000000000001"
    }


def test_source_binding_falls_back_to_legacy_top_level_keys() -> None:
    config = {
        "organization_id": "00D000000000001",
        "instance_url": "https://acme.my.salesforce.com",
    }
    assert SalesforceConnector._source_binding(config) == {
        "organization_id": "00D000000000001",
        "instance_url": "https://acme.my.salesforce.com",
    }


@pytest.mark.asyncio
async def test_validate_binds_first_seen_organization_id() -> None:
    connector = SalesforceConnector()
    binding = await connector.validate_oauth_credential(
        _source({}),
        {"instance_url": "https://acme.my.salesforce.com"},
        OAuthCredentialFlow.ORG_SOURCE,
        {"organization_id": "00D000000000001"},
    )
    assert _binding_dict(binding) == {"organization_id": "00D000000000001"}


@pytest.mark.asyncio
async def test_validate_rejects_mismatched_organization_id() -> None:
    connector = SalesforceConnector()
    with pytest.raises(ValueError, match="organization does not match"):
        await connector.validate_oauth_credential(
            _source({"source_binding": {"organization_id": "00D000000000001"}}),
            {"instance_url": "https://acme.my.salesforce.com"},
            OAuthCredentialFlow.USER_READ,
            {"organization_id": "00D000000000002"},
        )


@pytest.mark.asyncio
async def test_validate_rejects_mismatched_instance() -> None:
    connector = SalesforceConnector()
    with pytest.raises(ValueError, match="instance does not match"):
        await connector.validate_oauth_credential(
            _source(
                {
                    "source_binding": {
                        "organization_id": "00D000000000001",
                        "instance_url": "https://acme.my.salesforce.com",
                    }
                }
            ),
            {
                "instance_url": "https://other.my.salesforce.com",
                "organization_id": "00D000000000001",
            },
            OAuthCredentialFlow.USER_WRITE,
            {"organization_id": "00D000000000001"},
        )


@pytest.mark.asyncio
async def test_validate_rejects_credential_org_assertion_mismatch() -> None:
    """A credential's own organization_id is an assertion, never proof."""
    connector = SalesforceConnector()
    with pytest.raises(ValueError, match="does not match the credential"):
        await connector.validate_oauth_credential(
            _source({}),
            {
                "instance_url": "https://acme.my.salesforce.com",
                "organization_id": "00D000000000002",
            },
            OAuthCredentialFlow.USER_READ,
            {"organization_id": "00D000000000001"},
        )


@pytest.mark.asyncio
async def test_validate_does_not_trust_credential_org_without_userinfo(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Without verified userinfo the credential must be verified against the
    provider, even when the credential carries an organization id."""
    import salesforce_connector.connector as connector_module

    calls = 0

    async def fake_fetch_organization_id(auth: object) -> str:
        nonlocal calls
        calls += 1
        return "00D000000000001"

    monkeypatch.setattr(connector_module, "fetch_organization_id", fake_fetch_organization_id)

    connector = SalesforceConnector()
    binding = await connector.validate_oauth_credential(
        _source({"source_binding": {"organization_id": "00D000000000001"}}),
        {
            "access_token": "token",
            "instance_url": "https://acme.my.salesforce.com",
            "organization_id": "00D000000000001",
        },
        OAuthCredentialFlow.USER_READ,
        {},
    )
    assert calls == 1
    assert _binding_dict(binding) == {"organization_id": "00D000000000001"}


@pytest.mark.asyncio
async def test_validate_rejects_unverifiable_credential_that_asserts_org(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import salesforce_connector.connector as connector_module

    async def failing_fetch(auth: object) -> str:
        raise connector_module.SalesforceClientError("userinfo failed")

    monkeypatch.setattr(connector_module, "fetch_organization_id", failing_fetch)

    connector = SalesforceConnector()
    with pytest.raises(ValueError, match="could not be verified"):
        await connector.validate_oauth_credential(
            _source({}),
            {
                "access_token": "token",
                "instance_url": "https://acme.my.salesforce.com",
                "organization_id": "00D000000000001",
            },
            OAuthCredentialFlow.USER_READ,
            {},
        )


@pytest.mark.asyncio
async def test_validate_derives_organization_id_from_org_credential(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Setup-time org credentials (JWT / static token) carry no userinfo
    metadata; the connector must derive the org identity from the credential
    itself so the admin-owned binding is established during setup."""
    import salesforce_connector.connector as connector_module

    async def fake_fetch_organization_id(auth: object) -> str:
        return "00D000000000001"

    monkeypatch.setattr(
        connector_module, "fetch_organization_id", fake_fetch_organization_id
    )

    connector = SalesforceConnector()
    binding = await connector.validate_oauth_credential(
        _source({}),
        {"client_id": "cid", "private_key": "pem", "username": "u@x.com"},
        OAuthCredentialFlow.ORG_SOURCE,
        {},
    )
    assert _binding_dict(binding) == {"organization_id": "00D000000000001"}


@pytest.mark.asyncio
async def test_validate_fails_when_org_credential_cannot_be_verified(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import salesforce_connector.connector as connector_module

    async def failing_fetch(auth: object) -> str:
        raise connector_module.SalesforceClientError("userinfo failed")

    monkeypatch.setattr(connector_module, "fetch_organization_id", failing_fetch)

    connector = SalesforceConnector()
    with pytest.raises(ValueError, match="could not be verified"):
        await connector.validate_oauth_credential(
            _source({}),
            {"client_id": "cid", "private_key": "pem", "username": "u@x.com"},
            OAuthCredentialFlow.ORG_SOURCE,
            {},
        )
