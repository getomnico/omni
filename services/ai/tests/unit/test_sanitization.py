"""Source-level sanitization regression tests for AI service logs.

Tests scan the actual source files for forbidden patterns in logger.* call
lines.  Only lines that contain a recognised logging call are inspected, so
functional non-log code is not flagged.

This is a static analysis approach — no runtime dependencies required.
"""

import re
import ast
import os
from pathlib import Path


AI_SERVICE_DIR = Path(__file__).resolve().parent.parent.parent  # services/ai


def _is_log_call(line: str) -> bool:
    """Check if a line appears to be a logging call or continuation."""
    stripped = line.strip()
    # Match logger.info(, logger.debug(, logger.error(, logger.warn(, logger.exception(
    if re.search(r'logger\.(info|debug|error|warn|exception)\(', stripped):
        return True
    # Continuation lines inside a log call (indented and part of a call)
    if stripped.startswith('f"') or stripped.startswith('"') or stripped.startswith("'"):
        return True
    if stripped.startswith('+') or stripped == '':
        return False
    return False


def _get_forbidden_patterns() -> dict[str, list[str]]:
    """Return file-relative-path -> list of forbidden regex patterns."""
    return {
        "streaming/generate.py": [
            r"chat_id",                # chat IDs in log messages
            r"event.to_json",           # raw event dumps
            r"event\.delta\.text",       # text content in deltas
            r"event\.citation",          # citation content
            r"content_block\.text",      # text content
            r"content_block\.id",        # content block ID
            r"\{e\}",                    # exception string in f-string
            r"str\(e\)",                # exception string in log
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "streaming/run.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "providers/anthropic.py": [
            r"t\['name'\]",           # tool names
            r"event\.content_block\.input",  # tool input
            r"event\.citation",        # citation content
            r"text_delta.*event\.delta\.text",  # text content
            r"partial_json",           # JSON delta content
            r"input_json",             # input JSON
            r"json\.dumps.*messages",  # raw messages
            r"json\.dumps.*request_params",  # full request params
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "providers/bedrock.py": [
            r"t\['name'\]",           # tool names
            r"event\.content_block\.input",  # tool input
            r"event\.citation",        # citation content
            r"text_delta.*event\.delta\.text",  # text content
            r"partial_json",           # JSON delta content
            r"input_json",             # input JSON
            r"json\.dumps.*messages",  # raw messages
            r"json\.dumps.*request_body",  # full request body
            r"response_body",
            r"response from LLM ->",   # full response
            r"document\['name'\]",     # raw document name in log
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "providers/gemini.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "providers/openai_compatible.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "providers/openai.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "tools/searcher_client.py": [
            r"query:\s*\{request\.query",  # raw query in log
            r"query:\s*\{query",           # raw query variable
            r"response\.text",            # raw response body
            r"response\.status_code",     # only status code is acceptable
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "tools/connector_handler.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "tools/search_handler.py": [
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "email_service/sender.py": [
            r"body=%s",
            r"resp\.text",              # response body
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "embeddings/batch_processor.py": [
            r"document_id",              # document IDs in log messages
            r"\{e\}",                    # exception string in f-string
            r"str\(e\)",                # exception string in log
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
        "main.py": [
            r"\{e\}",                    # exception string in f-string
            r"exc_info\s*=\s*True",        # traceback export in log
        ],
    }


def _run_checks(rel_path: str, *, skip_line: callable = None) -> None:
    """Run forbidden-pattern checks for a given relative source path.

    Optional *skip_line* callable receives (line_text, pattern) and returns
    True if the match should be skipped.
    """
    filepath = AI_SERVICE_DIR / rel_path
    assert filepath.exists(), f"{filepath} not found"
    content = filepath.read_text()
    patterns = _get_forbidden_patterns()[rel_path]
    violations = []
    for i, line in enumerate(content.splitlines(), 1):
        if not _is_log_call(line):
            continue
        for pat in patterns:
            if re.search(pat, line):
                if skip_line is not None and skip_line(line, pat):
                    continue
                violations.append(f"  line {i}: {line.strip()}")
    assert not violations, (
        f"Found {len(violations)} forbidden pattern(s) in {rel_path} log lines:\n"
        + "\n".join(violations)
    )


def test_anthropic_sanitization():
    """Anthropic provider must not log tool input, text deltas, citations, request params, or tracebacks."""
    _run_checks("providers/anthropic.py")


def test_bedrock_sanitization():
    """Bedrock provider must not log full request body, messages, stream content, responses, raw document names, or tracebacks."""
    _run_checks("providers/bedrock.py")


def test_gemini_sanitization():
    """Gemini provider must not log tracebacks."""
    _run_checks("providers/gemini.py")


def test_openai_compatible_sanitization():
    """OpenAI-compatible provider must not log tracebacks."""
    _run_checks("providers/openai_compatible.py")


def test_openai_sanitization():
    """OpenAI provider must not log tracebacks."""
    _run_checks("providers/openai.py")


def test_streaming_generate_sanitization():
    """Streaming generate.py must not log chat IDs, raw events, text/citation content, exception strings, or tracebacks."""
    _run_checks("streaming/generate.py")


def test_streaming_run_sanitization():
    """Streaming run.py must not log tracebacks."""
    _run_checks("streaming/run.py")


def _skip_status_pattern(line: str, pat: str) -> bool:
    """Skip response.status_code matches when the line uses status= format."""
    if "status=" in line:
        return True
    return False


def test_searcher_client_sanitization():
    """Searcher client must not log raw queries, response bodies, or tracebacks."""
    _run_checks("tools/searcher_client.py", skip_line=_skip_status_pattern)


def test_connector_handler_sanitization():
    """Connector handler must not log tracebacks."""
    _run_checks("tools/connector_handler.py")


def test_search_handler_sanitization():
    """Search handler must not log tracebacks."""
    _run_checks("tools/search_handler.py")


def test_email_sender_sanitization():
    """Email sender must not log response body or tracebacks."""
    _run_checks("email_service/sender.py")


def _skip_batch_processor_non_log(line: str, pat: str) -> bool:
    """Skip data-structure lines that are not log calls."""
    # Line inside a data dict literal (not a log continuation)
    if pat == r"document_id" and '"document_id"' in line:
        return True
    return False


def test_batch_processor_sanitization():
    """Batch processor must not log document IDs, exception values, or tracebacks."""
    _run_checks("embeddings/batch_processor.py", skip_line=_skip_batch_processor_non_log)


def test_main_sanitization():
    """Main module must not log exception values or tracebacks."""
    _run_checks("main.py")
