"""Unit tests for the OAuth-required envelope encoding and the
ConnectorToolHandler 412 → oauth_required tool result path.
"""

from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.omni_tool_result import (
    OAuthRequiredPayload,
    OmniToolResultKind,
    encode_oauth_required,
    try_parse_envelope,
)
from tools.registry import ToolContext


pytestmark = pytest.mark.unit


class TestOmniToolResultEnvelope:
    def test_round_trip(self):
        payload = OAuthRequiredPayload(
            source_id="src-1",
            source_type="gmail",
            provider="google",
            oauth_start_url="/api/oauth/start?source_id=src-1",
        )
        block = encode_oauth_required(payload)
        assert block["type"] == "text"

        parsed = try_parse_envelope(block["text"])
        assert parsed is not None
        kind, raw = parsed
        assert kind is OmniToolResultKind.OAUTH_REQUIRED
        assert raw == {
            "source_id": "src-1",
            "source_type": "gmail",
            "provider": "google",
            "oauth_start_url": "/api/oauth/start?source_id=src-1",
        }

    def test_parse_rejects_non_envelope_text(self):
        assert try_parse_envelope("just plain text") is None
        assert try_parse_envelope('{"unrelated": "json"}') is None
        assert (
            try_parse_envelope('{"omni_kind": "unknown_kind", "payload": {}}') is None
        )
        assert try_parse_envelope("") is None


def _register_action(handler: ConnectorToolHandler, source_id: str) -> None:
    """Force a fake gmail__send_email action so the handler can dispatch."""
    handler._actions["gmail__send_email"] = ConnectorAction(
        source_id=source_id,
        source_type="gmail",
        source_name="Test Gmail",
        action_name="send_email",
        description="Send an email",
        input_schema={"type": "object", "properties": {}},
        mode="write",
    )
    handler._initialized = True


class TestConnectorHandlerOAuthRequired:
    @pytest.mark.asyncio
    async def test_412_response_produces_structured_oauth_required(self):
        """A 412 needs_user_auth response from connector-manager must yield a
        ToolResult whose content carries an oauth_required envelope and whose
        oauth_required field is the typed payload (so chat.py can pause)."""
        handler = ConnectorToolHandler(
            connector_manager_url="http://cm.test",
            user_id="user-1",
        )
        _register_action(handler, source_id="src-1")

        body = {
            "error": "needs_user_auth",
            "source_id": "src-1",
            "source_type": "gmail",
            "provider": "google",
            "oauth_start_url": "/api/oauth/start?source_id=src-1",
        }
        with respx.mock(assert_all_called=True) as mock:
            mock.post("http://cm.test/action").mock(
                return_value=Response(412, json=body)
            )
            result = await handler.execute(
                "gmail__send_email",
                {"to": "x@y.com"},
                ToolContext(chat_id="c1", user_id="user-1"),
            )

        assert result.is_error is False
        assert result.oauth_required is not None
        assert result.oauth_required.source_id == "src-1"
        assert result.oauth_required.source_type == "gmail"
        assert result.oauth_required.provider == "google"
        assert (
            result.oauth_required.oauth_start_url == "/api/oauth/start?source_id=src-1"
        )

        assert len(result.content) == 1
        envelope_text = result.content[0]["text"]
        parsed = try_parse_envelope(envelope_text)
        assert parsed is not None
        kind, raw = parsed
        assert kind is OmniToolResultKind.OAUTH_REQUIRED
        assert raw["source_id"] == "src-1"
        assert raw["source_type"] == "gmail"
        assert raw["provider"] == "google"
        assert raw["oauth_start_url"] == "/api/oauth/start?source_id=src-1"

    @pytest.mark.asyncio
    async def test_normal_200_does_not_set_oauth_required(self):
        handler = ConnectorToolHandler(
            connector_manager_url="http://cm.test",
            user_id="user-1",
        )
        _register_action(handler, source_id="src-1")

        with respx.mock(assert_all_called=True) as mock:
            mock.post("http://cm.test/action").mock(
                return_value=Response(
                    200,
                    json={"status": "ok", "result": {"message_id": "abc"}},
                )
            )
            result = await handler.execute(
                "gmail__send_email",
                {"to": "x@y.com"},
                ToolContext(chat_id="c1", user_id="user-1"),
            )

        assert result.oauth_required is None
        assert result.is_error is False
        assert "abc" in result.content[0]["text"]
