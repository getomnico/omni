"""Security contracts for upgraded AI-service dependencies, without network calls."""

from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

import jwt
import pytest
from hpack.exceptions import HPACKDecodingError
from hpack.hpack import decode_integer
from mem0.memory.main import _build_session_scope
from mem0.memory.storage import SQLiteManager
from opentelemetry.trace import TraceState
from tokenizers import Tokenizer
from tokenizers.models import WordLevel
from tokenizers.pre_tokenizers import Whitespace
from transformers import AutoTokenizer, PreTrainedTokenizerFast

from memory.providers.mem0.provider import Mem0Provider
from processing import Chunker

pytestmark = pytest.mark.unit


def test_mem0_session_scopes_do_not_collide():
    victim = _build_session_scope({"user_id": "alice", "run_id": "proj-x"})
    attacker = _build_session_scope({"run_id": "proj-x&user_id=alice"})
    encoded_attacker = _build_session_scope({"run_id": "proj-x%26user_id%3Dalice"})
    assert len({victim, attacker, encoded_attacker}) == 3


@pytest.mark.parametrize("key", ["user:u1", "user:a&b=c%26", "agent:a=b&c"])
def test_mem0_history_cleanup_uses_escaped_scope(tmp_path: Path, key: str):
    history = SQLiteManager(str(tmp_path / "history.db"))
    scope = _build_session_scope({"user_id": key})
    other_scope = _build_session_scope({"user_id": f"{key}-other"})
    messages = [{"role": "user", "content": "private fact"}]
    try:
        history.save_messages(messages, scope)
        history.save_messages(messages, other_scope)
        provider = Mem0Provider(SimpleNamespace(db=history), MagicMock())

        provider._clear_sqlite_history(key)

        assert history.get_last_messages(scope) == []
        assert len(history.get_last_messages(other_scope)) == 1
    finally:
        history.connection.close()


def test_jwt_unverified_decode_does_not_disable_later_claim_checks():
    secret = "a-test-secret-with-at-least-32-bytes"
    token = jwt.encode({"sub": "alice", "exp": 1}, secret, algorithm="HS256")
    options = {"verify_signature": False}
    jwt.decode(token, options=options)
    assert options == {"verify_signature": False}

    options["verify_signature"] = True
    with pytest.raises(jwt.ExpiredSignatureError):
        jwt.decode(token, secret, algorithms=["HS256"], options=options)


def test_jwt_deep_unsigned_payload_raises_documented_error():
    header = jwt.utils.base64url_encode(b'{"alg":"none"}')
    depth = 20_000
    payload = b'{"nested":' + b"[" * depth + b"0" + b"]" * depth + b"}"
    token = header + b"." + jwt.utils.base64url_encode(payload) + b"."
    with pytest.raises(jwt.DecodeError):
        jwt.decode(token, options={"verify_signature": False})


def test_tracestate_rejects_oversized_input_and_preserves_valid_state():
    assert list(TraceState.from_header(["vendor=" + " " * 8192]).items()) == []
    assert list(TraceState.from_header(["vendor=value"]).items()) == [("vendor", "value")]


def test_hpack_rejects_overlong_variable_integer():
    with pytest.raises(HPACKDecodingError):
        decode_integer(b"\xff" * 16, 7)


@pytest.fixture
def local_tokenizer() -> PreTrainedTokenizerFast:
    backend = Tokenizer(WordLevel({"[UNK]": 0, "Hello": 1, "world": 2, ".": 3}, unk_token="[UNK]"))
    backend.pre_tokenizer = Whitespace()
    return PreTrainedTokenizerFast(tokenizer_object=backend, unk_token="[UNK]")


@pytest.mark.parametrize("name", ["../outside", "../../outside", "nested/template"])
def test_tokenizer_refuses_chat_template_path_traversal(
    tmp_path: Path, local_tokenizer: PreTrainedTokenizerFast, name: str
):
    local_tokenizer.chat_template = {name: "untrusted template"}
    with pytest.raises(ValueError):
        local_tokenizer.save_pretrained(tmp_path / "tokenizer")
    assert not list(tmp_path.rglob("*.jinja"))


def test_tokenizer_roundtrip_and_chunking_without_remote_code(
    tmp_path: Path, local_tokenizer: PreTrainedTokenizerFast
):
    local_tokenizer.save_pretrained(tmp_path)
    tokenizer = AutoTokenizer.from_pretrained(tmp_path, local_files_only=True, trust_remote_code=False)
    text = "Hello world. Hello world."
    chunker = Chunker()
    for method in (chunker.chunk_by_tokens, chunker.chunk_by_sentences):
        token_spans, char_spans = method(text, 3, tokenizer)
        assert token_spans == [(0, 3), (3, 6)]
        assert [text[start:end].strip() for start, end in char_spans] == ["Hello world.", "Hello world."]
