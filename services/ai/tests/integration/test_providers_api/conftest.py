"""Shared conftest for provider API integration tests.

Loads .env so API keys are available before any test is collected, and prints
a cumulative token-usage summary at session end.
"""

import json
import os
import tempfile
from pathlib import Path

import pytest
from dotenv import load_dotenv

# Load .env before any test is collected
_repo_root = Path(__file__).resolve().parents[5]
_service_root = Path(__file__).resolve().parents[3]
load_dotenv(_repo_root / ".env")
load_dotenv(_service_root / ".env")


# ---------------------------------------------------------------------------
# Session-level token usage tracker — shared via a temp file to avoid
# module-identity issues with pytest's conftest loading.
# ---------------------------------------------------------------------------

_USAGE_FILE = os.path.join(tempfile.gettempdir(), "omni_provider_test_usage.json")


def _load_usage() -> list[dict]:
    try:
        with open(_USAGE_FILE) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return []


def _save_usage(entries: list[dict]) -> None:
    with open(_USAGE_FILE, "w") as f:
        json.dump(entries, f)


def report_usage(test_name: str, input_tokens: int, output_tokens: int) -> None:
    entries = _load_usage()
    entries.append({"test": test_name, "input": input_tokens, "output": output_tokens})
    _save_usage(entries)


def _format_summary(entries: list[dict]) -> str:
    if not entries:
        return ""
    total_in = sum(e["input"] for e in entries)
    total_out = sum(e["output"] for e in entries)
    lines = [
        "",
        "=" * 66,
        "  Token Usage Summary",
        "=" * 66,
        f"  {'Test':<36} {'Input':>8} {'Output':>8}",
        "  " + "-" * 54,
    ]
    for e in entries:
        lines.append(f"  {e['test']:<36} {e['input']:>8} {e['output']:>8}")
    lines.append("  " + "-" * 54)
    lines.append(f"  {'TOTAL':<36} {total_in:>8} {total_out:>8}")
    lines.append(f"  Calls: {len(entries)}")
    lines.append("=" * 66)
    return "\n".join(lines)


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    """Print cumulative token usage across all real-LLM tests."""
    entries = _load_usage()
    summary = _format_summary(entries)
    if summary:
        print(summary)


# ---------------------------------------------------------------------------
# Convenience helpers used by test files
# ---------------------------------------------------------------------------


def require_env(key: str) -> str:
    """Return the env var value or raise a clear ``RuntimeError``."""
    value = os.environ.get(key)
    if not value or value.strip() == "":
        raise RuntimeError(
            f"Missing required environment variable '{key}'. "
            f"Set it in .env and re-run the tests."
        )
    return value


def assert_usage(
    usage: object,
    *,
    max_output_tokens: int | None = None,
    tag: str = "",
) -> None:
    """Assert a TokenUsage-like object has sensible token counts."""
    input_tokens = getattr(usage, "input_tokens", None)
    output_tokens = getattr(usage, "output_tokens", None)

    assert input_tokens is not None and input_tokens > 0, (
        f"{tag}expected positive input_tokens, got {input_tokens}"
    )
    assert output_tokens is not None and output_tokens > 0, (
        f"{tag}expected positive output_tokens, got {output_tokens}"
    )

    if max_output_tokens is not None:
        assert output_tokens <= max_output_tokens, (
            f"{tag}output_tokens={output_tokens} exceeds "
            f"max_output_tokens={max_output_tokens}"
        )


def stream_usage(events: list[object]) -> tuple[int, int] | None:
    """Extract ``(input_tokens, output_tokens)`` from a stream's ``message_delta``."""
    for event in events:
        typ = getattr(event, "type", None)
        if typ == "message_delta":
            u = getattr(event, "usage", None)
            if u is not None:
                inp = getattr(u, "input_tokens", 0) or 0
                out = getattr(u, "output_tokens", 0) or 0
                if inp > 0 and out > 0:
                    return (inp, out)
    return None
