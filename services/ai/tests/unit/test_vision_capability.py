"""Unit tests for vision-capability resolution (vision_capability.py)."""

from __future__ import annotations

import pytest

from vision_capability import (
    VISION_MODE_AUTO,
    VISION_MODE_OFF,
    VISION_MODE_ON,
    curated_vision_capable,
    effective_vision,
    parse_vision_mode,
)


@pytest.mark.parametrize(
    "model_id",
    [
        "gpt-4o",
        "gpt-4o-mini-2024-07-18",
        "claude-sonnet-4-5",
        "claude-3-5-haiku-latest",
        "gemini-2.0-flash",
        "openai/gpt-4.1",
        "meta-llama/llama-4-scout",
        "qwen2-vl-7b-instruct",
    ],
)
def test_curated_map_recognizes_vision_models(model_id: str):
    assert curated_vision_capable(model_id)


@pytest.mark.parametrize(
    "model_id",
    ["", None, "gpt-3.5-turbo", "claude-2.1", "gemini-1.0-pro", "text-embedding-3-small"],
)
def test_curated_map_rejects_non_vision_models(model_id):
    assert not curated_vision_capable(model_id)


def test_case_and_whitespace_insensitive():
    assert curated_vision_capable("  GPT-4O  ")
    assert curated_vision_capable("Claude-Sonnet-4")


def test_parse_vision_mode_lenient():
    assert parse_vision_mode("on") == VISION_MODE_ON
    assert parse_vision_mode(" OFF ") == VISION_MODE_OFF
    assert parse_vision_mode("Auto") == VISION_MODE_AUTO
    assert parse_vision_mode("sometimes") == VISION_MODE_AUTO
    assert parse_vision_mode(None) == VISION_MODE_AUTO
    assert parse_vision_mode(42) == VISION_MODE_AUTO


def test_override_wins_over_curated_map():
    # off beats a vision-capable model; on rescues a non-vision one
    assert not effective_vision("off", "openai_compatible", "gpt-4o")
    assert effective_vision("on", "openai_compatible", "gpt-3.5-turbo")


def test_auto_defers_to_curated_map():
    assert effective_vision("auto", "anthropic", "claude-sonnet-4-5")
    assert not effective_vision("auto", "openai_compatible", "gpt-3.5-turbo")
    assert effective_vision(None, "gemini", "gemini-2.0-flash")
    assert effective_vision("garbage", "vertex_ai", "claude-opus-4-1")
