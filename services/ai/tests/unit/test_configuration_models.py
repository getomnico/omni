"""Unit tests for typed configuration model parsing."""

import pytest

from db.models import DoclingQualityPreset, GlobalConfiguration, UserConfiguration
from memory import MemoryMode


@pytest.mark.unit
class TestUserConfiguration:
    def test_from_rows_decodes_asyncpg_json_string_values(self):
        configuration = UserConfiguration.from_rows(
            [
                {"key": "timezone", "value": '{"value": "Asia/Bahrain"}'},
                {"key": "memory_mode", "value": '{"value": "chat"}'},
            ]
        )

        assert configuration is not None
        assert configuration.timezone == "Asia/Bahrain"
        assert configuration.memory_mode == MemoryMode.CHAT


@pytest.mark.unit
class TestGlobalConfiguration:
    def test_from_rows_decodes_asyncpg_json_string_values(self):
        configuration = GlobalConfiguration.from_rows(
            [
                {"key": "docling_enabled", "value": '{"enabled": true}'},
                {"key": "docling_quality_preset", "value": '{"preset": "quality"}'},
                {"key": "memory_mode_default", "value": '{"value": "full"}'},
                {"key": "memory_llm_id", "value": '{"value": "model-1"}'},
            ]
        )

        assert configuration.docling_enabled is True
        assert configuration.docling_quality_preset == DoclingQualityPreset.QUALITY
        assert configuration.memory_mode_default == MemoryMode.FULL
        assert configuration.memory_llm_id == "model-1"
