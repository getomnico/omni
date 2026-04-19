"""Unit tests for memory mode resolution logic."""
import pytest
from memory.mode import resolve_memory_mode


@pytest.mark.unit
class TestResolveMemoryMode:
    def test_user_lower_than_org_wins(self):
        # User chose chat, org allows full — user wins because chat ≤ full
        assert resolve_memory_mode(user_mode="chat", org_default="full") == "chat"

    def test_user_off_wins_over_org_full(self):
        # Users can always dial down to off
        assert resolve_memory_mode(user_mode="off", org_default="full") == "off"

    def test_none_user_inherits_org_default(self):
        assert resolve_memory_mode(user_mode=None, org_default="chat") == "chat"

    def test_none_user_none_org_defaults_to_off(self):
        assert resolve_memory_mode(user_mode=None, org_default=None) == "off"

    def test_org_default_full_with_no_user_override(self):
        assert resolve_memory_mode(user_mode=None, org_default="full") == "full"

    def test_invalid_user_mode_treated_as_off(self):
        # Defensive: bad column value should not silently grant memory
        assert resolve_memory_mode(user_mode="unknown", org_default="chat") == "off"

    # Ceiling semantics
    def test_user_full_capped_by_org_chat(self):
        assert resolve_memory_mode(user_mode="full", org_default="chat") == "chat"

    def test_user_chat_capped_by_org_off(self):
        assert resolve_memory_mode(user_mode="chat", org_default="off") == "off"

    def test_user_full_capped_by_org_off(self):
        assert resolve_memory_mode(user_mode="full", org_default="off") == "off"

    def test_invalid_org_default_treated_as_off_ceiling(self):
        # If org value is garbage, the ceiling collapses to off
        assert resolve_memory_mode(user_mode="full", org_default="nonsense") == "off"
