"""Unit tests for project instructions/context injection in chat system prompts."""

import prompts


def _sources():
    return []


def test_chat_prompt_without_project_is_unchanged():
    without_project = prompts.build_chat_system_prompt(_sources())
    assert "Project instructions" not in without_project
    assert "Project context" not in without_project


def test_chat_prompt_includes_project_instructions():
    prompt = prompts.build_chat_system_prompt(
        _sources(), project_instructions="Always answer in German."
    )
    assert "# Project instructions" in prompt
    assert "Always answer in German." in prompt
    assert "Apply them" in prompt


def test_chat_prompt_includes_project_context_lines():
    lines = ["Q3 Roadmap [_ref:01HXXXXXX]"]
    prompt = prompts.build_chat_system_prompt(_sources(), project_context_lines=lines)
    assert "# Project context" in prompt
    assert "- Q3 Roadmap [_ref:01HXXXXXX]" in prompt
    assert "read_document" in prompt


def test_chat_prompt_orders_project_sections_before_memories():
    prompt = prompts.build_chat_system_prompt(
        _sources(),
        project_instructions="Be terse.",
        project_context_lines=["Spec [_ref:01HYYYYYY]"],
        memories=["User prefers email."],
    )
    instructions_at = prompt.index("# Project instructions")
    context_at = prompt.index("# Project context")
    memory_at = prompt.index("<untrusted-memory>")
    assert instructions_at < context_at < memory_at


def test_chat_prompt_empty_context_lines_renders_no_section():
    prompt = prompts.build_chat_system_prompt(
        _sources(), project_instructions=None, project_context_lines=[]
    )
    assert "Project instructions" not in prompt
    assert "Project context" not in prompt
