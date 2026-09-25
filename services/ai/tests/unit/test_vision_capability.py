"""Unit tests for vision-capability resolution (vision_capability.py)."""

from __future__ import annotations

import pytest

from vision_capability import (
    VISION_MODE_AUTO,
    VISION_MODE_OFF,
    VISION_MODE_ON,
    effective_vision,
    parse_vision_mode,
    static_vision,
)


@pytest.mark.parametrize(
    "provider_type",
    ["anthropic", "gemini", "bedrock", "vertex_ai"],
)
def test_vision_families_resolve_without_probing(provider_type: str):
    assert static_vision(provider_type) is True
    assert effective_vision("auto", provider_type) is True


def test_metadata_provider_defers_to_endpoint_answer():
    assert static_vision("openai_compatible") is None
    assert effective_vision("auto", "openai_compatible", None) is None
    assert effective_vision("auto", "openai_compatible", True) is True
    assert effective_vision("auto", "openai_compatible", False) is False


@pytest.mark.parametrize("provider_type", ["openai", "azure_foundry", "mystery_type"])
def test_types_without_image_passthrough_default_off(provider_type: str):
    assert static_vision(provider_type) is False
    assert effective_vision("auto", provider_type, True) is False


def test_openai_vision_model_families_are_recognized():
    for model_id in ("gpt-5.6-luna", "gpt-4o", "gpt-4.1", "o3"):
        assert static_vision("openai", model_id) is True
        assert effective_vision("auto", "openai", model_id=model_id) is True

    assert static_vision("openai", "gpt-3.5-turbo") is False
    assert effective_vision("auto", "openai", model_id="gpt-3.5-turbo") is False


def test_override_wins_over_family_and_endpoint():
    assert effective_vision("off", "anthropic") is False
    assert effective_vision("on", "openai") is True
    assert effective_vision("on", "openai_compatible", False) is True
    assert effective_vision("off", "openai_compatible", True) is False


def test_parse_vision_mode_lenient():
    assert parse_vision_mode("on") == VISION_MODE_ON
    assert parse_vision_mode(" OFF ") == VISION_MODE_OFF
    assert parse_vision_mode("Auto") == VISION_MODE_AUTO
    assert parse_vision_mode("sometimes") == VISION_MODE_AUTO
    assert parse_vision_mode(None) == VISION_MODE_AUTO
    assert parse_vision_mode(42) == VISION_MODE_AUTO
