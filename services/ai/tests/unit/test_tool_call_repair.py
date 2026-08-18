"""Unit tests for interrupted-tool-call repair and resume semantics."""

from __future__ import annotations

from anthropic.types import MessageParam

from streaming.generate import (
    _INTERRUPTED_TOOL_RESULT_MARKER,
    repair_interrupted_tool_calls,
    resumable_batch_ids_for_interventions,
    strip_synthetic_interrupted_results,
)


def _tool_use(tool_id: str, name: str = "search") -> dict:
    return {"type": "tool_use", "id": tool_id, "name": name, "input": {}}


def _tool_result(tool_id: str, content: str = "result") -> dict:
    return {
        "type": "tool_result",
        "tool_use_id": tool_id,
        "content": [{"type": "text", "text": content}],
    }


def _interrupted_placeholder(tool_id: str) -> dict:
    return {
        "type": "tool_result",
        "tool_use_id": tool_id,
        "is_error": True,
        "content": [
            {
                "type": "text",
                "text": f"Tool call search {_INTERRUPTED_TOOL_RESULT_MARKER}. "
                "Treat this tool call as failed and retry it if the result is still needed.",
            }
        ],
    }


# ---------------------------------------------------------------------------
# repair_interrupted_tool_calls
# ---------------------------------------------------------------------------


def test_repair_does_not_flag_parallel_results_split_across_messages():
    """A parallel batch whose results are delivered over two consecutive user
    messages (one call delayed behind an approval) must not be marked
    interrupted: every call already has exactly one result."""
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
        {"role": "user", "content": [_tool_result("call_A")]},
        {"role": "user", "content": [_tool_result("call_B")]},
    ]

    repaired, count = repair_interrupted_tool_calls(messages)

    assert count == 0
    assert len(repaired) == len(messages)
    # No synthetic placeholder may be injected anywhere.
    for message in repaired:
        content = message["content"]
        if isinstance(content, list):
            for block in content:
                if block["type"] == "tool_result":
                    assert not block.get("is_error")


def test_repair_folds_placeholder_into_last_result_of_partial_batch():
    """A genuinely interrupted call in a partially answered batch gets a
    placeholder folded into the batch's last result message so the batch stays
    contiguous."""
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
        {"role": "user", "content": [_tool_result("call_A")]},
    ]

    repaired, count = repair_interrupted_tool_calls(messages)

    assert count == 1
    assert len(repaired) == 3
    last_message = repaired[-1]
    blocks = last_message["content"]
    assert [b["tool_use_id"] for b in blocks] == ["call_A", "call_B"]
    assert blocks[-1]["is_error"]


def test_repair_emits_standalone_placeholder_for_fully_unanswered_batch():
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
    ]

    repaired, count = repair_interrupted_tool_calls(messages)

    assert count == 2
    assert len(repaired) == 3
    placeholder_message = repaired[-1]
    assert placeholder_message["role"] == "user"
    assert [b["tool_use_id"] for b in placeholder_message["content"]] == [
        "call_A",
        "call_B",
    ]


def test_repair_respects_preserved_tool_call_ids():
    """Intervention-resumed calls are preserved and never placeholder'd."""
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
    ]

    repaired, count = repair_interrupted_tool_calls(
        messages, preserve_tool_call_ids={"call_A", "call_B"}
    )

    assert count == 0
    assert len(repaired) == len(messages)


# ---------------------------------------------------------------------------
# resumable_batch_ids_for_interventions
# ---------------------------------------------------------------------------


def test_terminal_batch_is_resumable():
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
        {"role": "user", "content": [_tool_result("call_A")]},
    ]

    assert resumable_batch_ids_for_interventions(messages, {"call_B"}) == {
        "call_A",
        "call_B",
    }


def test_batch_followed_by_newer_user_message_is_not_resumable():
    """A newer conversational turn after the batch means the stale call must be
    marked interrupted instead of resumed with its result appended after the
    newer turn."""
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
        {"role": "user", "content": [_tool_result("call_A")]},
        {"role": "user", "content": "actually, do this instead"},
    ]

    assert resumable_batch_ids_for_interventions(messages, {"call_B"}) == set()


def test_no_interventions_means_nothing_resumable():
    messages: list[MessageParam] = [
        {"role": "user", "content": "go"},
        {
            "role": "assistant",
            "content": [_tool_use("call_A")],
        },
    ]

    assert resumable_batch_ids_for_interventions(messages, set()) == set()


# ---------------------------------------------------------------------------
# strip_synthetic_interrupted_results
# ---------------------------------------------------------------------------


def test_strip_removes_merged_placeholder_and_keeps_real_results():
    messages: list[MessageParam] = [
        {
            "role": "assistant",
            "content": [_tool_use("call_A"), _tool_use("call_B")],
        },
        {
            "role": "user",
            "content": [_tool_result("call_A"), _interrupted_placeholder("call_B")],
        },
    ]

    stripped = strip_synthetic_interrupted_results(messages, {"call_B"})

    blocks = stripped[-1]["content"]
    assert [b["tool_use_id"] for b in blocks] == ["call_A"]


def test_strip_drops_standalone_placeholder_message():
    messages: list[MessageParam] = [
        {
            "role": "assistant",
            "content": [_tool_use("call_A")],
        },
        {"role": "user", "content": [_interrupted_placeholder("call_A")]},
        {"role": "user", "content": "new question"},
    ]

    stripped = strip_synthetic_interrupted_results(messages, {"call_A"})

    assert len(stripped) == 2
    assert stripped[-1] == {"role": "user", "content": "new question"}


def test_strip_leaves_unrelated_results_untouched():
    messages: list[MessageParam] = [
        {
            "role": "user",
            "content": [_tool_result("call_X"), _interrupted_placeholder("call_Y")],
        },
    ]

    stripped = strip_synthetic_interrupted_results(messages, {"call_Y"})

    blocks = stripped[-1]["content"]
    assert [b["tool_use_id"] for b in blocks] == ["call_X"]
