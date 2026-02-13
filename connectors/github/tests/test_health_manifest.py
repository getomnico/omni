"""Integration tests: connector health checks and manifest via connector-manager."""

import pytest
import httpx

pytestmark = pytest.mark.integration


async def test_connector_listed_in_connectors(
    harness, cm_client: httpx.AsyncClient, connector_server: str
):
    resp = await cm_client.get("/connectors")
    assert resp.status_code == 200
    connectors = resp.json()

    github_entries = [c for c in connectors if c.get("source_type") == "github"]
    assert (
        len(github_entries) == 1
    ), f"Expected 1 github connector, got {len(github_entries)}: {connectors}"
    assert github_entries[0]["healthy"] is True


async def test_connector_manifest(
    harness, cm_client: httpx.AsyncClient, connector_server: str
):
    resp = await cm_client.get("/connectors")
    assert resp.status_code == 200
    connectors = resp.json()

    github_entries = [c for c in connectors if c.get("source_type") == "github"]
    assert len(github_entries) == 1
    manifest = github_entries[0]["manifest"]
    assert manifest["name"] == "github"
    assert manifest["version"] == "1.0.0"
    assert "full" in manifest["sync_modes"]
    assert "incremental" in manifest["sync_modes"]


async def test_direct_health_check(connector_server: str):
    async with httpx.AsyncClient() as client:
        resp = await client.get(f"{connector_server}/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "healthy"
