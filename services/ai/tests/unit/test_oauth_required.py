"""Unit tests for the OAuth-required envelope encoding and the
ConnectorToolHandler 412 → oauth_required tool result path.
"""

from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

import tools.connector_handler as connector_handler_module
from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.omni_tool_result import (
    OAuthRequiredPayload,
    OmniToolResultKind,
    encode_oauth_required,
)
from tools.registry import ToolContext

pytestmark = pytest.mark.unit


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


class _CredentialConnection:
    def __init__(self, credential: dict) -> None:
        self.credential = credential

    async def fetchrow(self, _query: str, *_args: object) -> dict:
        return self.credential


class _AcquireConnection:
    def __init__(self, credential: dict) -> None:
        self.connection = _CredentialConnection(credential)

    async def __aenter__(self) -> _CredentialConnection:
        return self.connection

    async def __aexit__(self, *_args: object) -> None:
        return None


class _CredentialPool:
    def __init__(self, credential: dict) -> None:
        self.credential = credential

    def acquire(self) -> _AcquireConnection:
        return _AcquireConnection(self.credential)


class TestConnectorHandlerOAuthRequired:
    @pytest.mark.asyncio
    async def test_existing_credential_missing_action_scope_requires_oauth(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        handler = ConnectorToolHandler(
            connector_manager_url="http://cm.test",
            user_id="user-1",
        )
        handler._actions["windshift__add_comment"] = ConnectorAction(
            source_id="src-1",
            source_type="windshift",
            source_name="Windshift",
            action_name="add_comment",
            description="Add a comment",
            input_schema={"type": "object", "properties": {}},
            mode="write",
            required_scopes=["items:write"],
        )
        handler._initialized = True

        async def fake_get_db_pool() -> _CredentialPool:
            return _CredentialPool(
                {
                    "id": "credential-1",
                    "provider": "windshift",
                    "config": json.dumps(
                        {"granted_scopes": ["mcp:access", "items:read"]}
                    ),
                }
            )

        monkeypatch.setattr(connector_handler_module, "get_db_pool", fake_get_db_pool)

        payload = await handler.check_oauth_required(
            "windshift__add_comment",
            {},
            ToolContext(chat_id="c1", user_id="user-1"),
        )

        assert payload is not None
        assert payload.oauth_start_url == (
            "/api/oauth/start?source_id=src-1&flow=user_write&"
            "required_scopes=items%3Awrite"
        )

    @pytest.mark.asyncio
    async def test_existing_credential_with_action_scope_does_not_require_oauth(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        handler = ConnectorToolHandler(
            connector_manager_url="http://cm.test",
            user_id="user-1",
        )
        handler._actions["windshift__add_comment"] = ConnectorAction(
            source_id="src-1",
            source_type="windshift",
            source_name="Windshift",
            action_name="add_comment",
            description="Add a comment",
            input_schema={"type": "object", "properties": {}},
            mode="write",
            required_scopes=["items:write"],
        )
        handler._initialized = True

        async def fake_get_db_pool() -> _CredentialPool:
            return _CredentialPool(
                {
                    "id": "credential-1",
                    "provider": "windshift",
                    "config": {
                        "granted_scopes": [
                            "mcp:access",
                            "items:read",
                            "items:write",
                        ]
                    },
                }
            )

        monkeypatch.setattr(connector_handler_module, "get_db_pool", fake_get_db_pool)

        payload = await handler.check_oauth_required(
            "windshift__add_comment",
            {},
            ToolContext(chat_id="c1", user_id="user-1"),
        )

        assert payload is None

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
        parsed = json.loads(envelope_text)
        assert parsed["omni_kind"] == OmniToolResultKind.OAUTH_REQUIRED.value
        assert parsed["payload"] == {
            "source_id": "src-1",
            "source_type": "gmail",
            "provider": "google",
            "oauth_start_url": "/api/oauth/start?source_id=src-1",
        }

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


class TestEncodeOAuthRequired:
    def test_encodes_envelope_with_payload(self):
        payload = OAuthRequiredPayload(
            source_id="src-1",
            source_type="gmail",
            provider="google",
            oauth_start_url="/api/oauth/start?source_id=src-1",
        )
        block = encode_oauth_required(payload)

        assert block["type"] == "text"
        envelope = json.loads(block["text"])
        assert envelope["omni_kind"] == OmniToolResultKind.OAUTH_REQUIRED.value
        assert envelope["payload"] == {
            "source_id": "src-1",
            "source_type": "gmail",
            "provider": "google",
            "oauth_start_url": "/api/oauth/start?source_id=src-1",
        }
