"""Unit tests for memory mode resolution logic."""

import pytest

from memory import MemoryMode, parse_org_default, resolve_memory_mode


@pytest.mark.unit
class TestResolveMemoryMode:
    def test_user_lower_than_org_wins(self):
        # User chose chat, org allows full — user wins because chat ≤ full
        assert resolve_memory_mode(MemoryMode.CHAT, MemoryMode.FULL) == MemoryMode.CHAT

    def test_user_off_wins_over_org_full(self):
        # Users can always dial down to off
        assert resolve_memory_mode(MemoryMode.OFF, MemoryMode.FULL) == MemoryMode.OFF

    def test_none_user_inherits_org_default(self):
        assert resolve_memory_mode(None, MemoryMode.CHAT) == MemoryMode.CHAT

    def test_none_user_off_org_returns_off(self):
        assert resolve_memory_mode(None, MemoryMode.OFF) == MemoryMode.OFF

    def test_org_default_full_with_no_user_override(self):
        assert resolve_memory_mode(None, MemoryMode.FULL) == MemoryMode.FULL

    def test_user_full_capped_by_org_chat(self):
        assert resolve_memory_mode(MemoryMode.FULL, MemoryMode.CHAT) == MemoryMode.CHAT

    def test_user_chat_capped_by_org_off(self):
        assert resolve_memory_mode(MemoryMode.CHAT, MemoryMode.OFF) == MemoryMode.OFF

    def test_user_full_capped_by_org_off(self):
        assert resolve_memory_mode(MemoryMode.FULL, MemoryMode.OFF) == MemoryMode.OFF


@pytest.mark.unit
class TestMemoryModeParse:
    def test_parses_known_strings(self):
        assert MemoryMode.parse("off") == MemoryMode.OFF
        assert MemoryMode.parse("chat") == MemoryMode.CHAT
        assert MemoryMode.parse("full") == MemoryMode.FULL

    def test_parse_is_case_insensitive(self):
        assert MemoryMode.parse("FULL") == MemoryMode.FULL

    def test_parses_none_and_empty_to_none(self):
        assert MemoryMode.parse(None) is None
        assert MemoryMode.parse("") is None

    def test_parses_unknown_to_none(self):
        assert MemoryMode.parse("nonsense") is None


@pytest.mark.unit
class TestParseOrgDefault:
    def test_value_key(self):
        assert parse_org_default({"value": "chat"}) == MemoryMode.CHAT

    def test_legacy_mode_key(self):
        assert parse_org_default({"mode": "full"}) == MemoryMode.FULL

    def test_value_key_takes_precedence_over_mode(self):
        assert parse_org_default({"value": "off", "mode": "full"}) == MemoryMode.OFF

    def test_none_returns_off(self):
        assert parse_org_default(None) == MemoryMode.OFF

    def test_empty_dict_returns_off(self):
        assert parse_org_default({}) == MemoryMode.OFF

    def test_unknown_value_returns_off(self):
        assert parse_org_default({"value": "nonsense"}) == MemoryMode.OFF
