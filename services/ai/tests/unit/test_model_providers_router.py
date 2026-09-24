"""Unit tests for the model-providers admin router."""

from __future__ import annotations

import pytest

from routers import model_providers


class _Record:
    id = "m1"
    model_id = "accounts/fw/deepseek-vision"
    display_name = "DeepSeek Vision"
    provider_type = "openai_compatible"
    is_default = True


class _FakeRepo:
    async def list_active(self):
        return [_Record()]


class _Resolved:
    def __init__(self, supports_vision: bool):
        self.supports_vision = supports_vision


class _FakeCache:
    def __init__(self, supports_vision: bool):
        self._supports_vision = supports_vision
        self.requested: list[str] = []

    async def resolve_for_model(self, model_record_id: str):
        self.requested.append(model_record_id)
        return _Resolved(self._supports_vision)


class _State:
    def __init__(self, cache):
        self.provider_cache = cache


class _App:
    def __init__(self, cache):
        self.state = _State(cache)


class _Request:
    def __init__(self, cache):
        self.app = _App(cache)


@pytest.mark.asyncio
async def test_list_models_reports_vision_capability(monkeypatch):
    cache = _FakeCache(supports_vision=True)
    monkeypatch.setattr(model_providers, "ModelsRepository", _FakeRepo)

    result = await model_providers.list_models(_Request(cache))

    assert cache.requested == ["m1"]
    assert result == [
        {
            "id": "m1",
            "modelId": "accounts/fw/deepseek-vision",
            "displayName": "DeepSeek Vision",
            "providerType": "openai_compatible",
            "isDefault": True,
            "supportsVision": True,
        }
    ]


@pytest.mark.asyncio
async def test_list_models_defaults_to_no_vision_when_unresolvable(monkeypatch):
    class _NoneCache:
        async def resolve_for_model(self, model_record_id: str):
            return None

    monkeypatch.setattr(model_providers, "ModelsRepository", _FakeRepo)

    result = await model_providers.list_models(_Request(_NoneCache()))

    assert result[0]["supportsVision"] is False


@pytest.mark.asyncio
async def test_catalog_endpoint_lists_full_catalog_shortlist_stays_small(monkeypatch):
    from routers import model_providers as mp

    async def fake_list(provider_type, provider, req, limit=3):
        return [mp.AvailableModel(model_id=str(i), display_name=str(i)) for i in range(limit)]

    monkeypatch.setattr(mp, "_list_provider_models", fake_list)
    monkeypatch.setattr(mp, "_build_provider", lambda provider_type, req: object())

    catalog = await mp.list_provider_model_catalog("openai_compatible", mp.TestModelRequest())
    shortlist = await mp.list_provider_models("openai_compatible", mp.TestModelRequest())

    assert len(catalog.models) == mp.DISCOVERY_CATALOG_LIMIT
    assert len(shortlist.models) == mp.DISCOVERED_MODELS_LIMIT


@pytest.mark.asyncio
async def test_list_endpoints_swallow_provider_errors(monkeypatch):
    from routers import model_providers as mp

    def boom(provider_type, req):
        raise RuntimeError("bad config")

    monkeypatch.setattr(mp, "_build_provider", boom)

    catalog = await mp.list_provider_model_catalog("openai_compatible", mp.TestModelRequest())
    shortlist = await mp.list_provider_models("openai_compatible", mp.TestModelRequest())

    assert catalog.models == []
    assert shortlist.models == []
