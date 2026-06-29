# Plan: Issue #340 — Expose MCP Resources and Prompts Through Agent Capability Search

## Context

Issue #340 asks for MCP resources and prompts from connector manifests to become first-class searchable/loadable agent capabilities, analogous to existing `tool_search`/`load_tool` and `skill_search`/`load_skill`. The implementation should avoid injecting all resources/prompts into the default prompt, preserve MCP-tool-only connector compatibility, and enforce current user/source credential access.

Current findings:
- `agent_capabilities` is intentionally open-ended (`capability_type TEXT`) in `services/migrations/099_create_agent_capabilities.sql`; no DB enum migration should be needed for `resource` or `prompt`.
- Searcher already provides generic capability upsert/search in `services/searcher/src/capabilities_repository.rs` and endpoint models in `services/searcher/src/models.rs` / `services/ai/tools/searcher_client.py`.
- Connector tool publishing happens dynamically in `services/ai/tools/meta_handler.py` from `ConnectorToolHandler.actions`; skill publishing happens dynamically in `services/ai/tools/skill_handler.py`.
- Connector-manager already lists and proxies MCP resources/prompts in `services/connector-manager/src/handlers.rs` (`/resources`, `/prompts`, `/resource`, `/prompt`) and `services/connector-manager/src/connector_client.rs`.
- Shared manifest/request models already include `McpResourceDefinition`, `McpPromptDefinition`, `ResourceRequest`, and `PromptRequest` in `shared/src/models.rs`.
- SDK adapters normalize MCP resource reads to `{contents: [...]}` and MCP prompts to structured `{description, messages}` responses in `sdk/python/omni_connector/mcp_adapter.py`, `sdk/typescript/src/mcp-adapter.ts`, and `sdk/rust/src/mcp_adapter.rs`; prompt message content is usually text, but the MCP shape is message-based rather than a single raw string.
- MCP docs confirm this is intentional: `prompts/get` returns `messages: PromptMessage[]`, where each message has `role` (`user` or `assistant`) and `content` (`text`, `image`, `audio`, or embedded `resource`). Official examples include both a single-string code-review prompt and a multi-message debug prompt with `UserMessage`, `UserMessage`, and `AssistantMessage`.
- Existing line-range UX exists for `read_document` in `services/ai/tools/document_handler.py` and can be mirrored for large MCP text resources.

## Approach

Recommended direction: add a dedicated MCP capability handler in the AI service that discovers accessible MCP resources/prompts from connector manifests, publishes them into `agent_capabilities`, and exposes always-on `resource_search`/`load_resource` plus `prompt_search`/`load_prompt` tools. Reuse the connector/source discovery and filtering pattern from `ConnectorToolHandler`, the capability search/upsert pattern from `MetaToolHandler`/`SkillHandler`, and the connector-manager proxy endpoints for loading.

For resources, `load_resource` should support optional `start_line` and `end_line` for text content. If the resource is small or a line range is provided, return inline text with line metadata. If it is too large and no range is provided, return a preview plus total/available line numbers and instruct the model to call `load_resource` again with a focused line range. Binary/blob content should not be dumped into the model context; return metadata and a clear unsupported/too-large message unless a later sandbox/export path is added.

For prompts, treat MCP prompts as structured prompt templates, not user-authored conversation history. This matches the MCP purpose: prompts define reusable interaction patterns, including few-shot turns, assistant scaffolding, multimodal context, and embedded server resources. `load_prompt` should return the connector-manager `{description, messages}` response as a structured, provenance-marked tool result. Preserve roles/order and content types inside that tool result so the model can apply the template, but do not inject those messages as mainline `user`/`assistant` conversation messages. Do not put prompt content in the system prompt; loading happens only on demand with validated arguments.

## Files to modify

Primary files:
- New `services/ai/tools/mcp_capability_handler.py` (name flexible): discovery, publishing, searching, loading, formatting.
- `services/ai/routers/chat.py`: register the new handler for normal chat and agent chat, passing current user/source filters.
- `services/ai/prompts.py`: add concise guidance for when to use resource/prompt search/load.
- `services/ai/tests/unit/test_mcp_capability_handler.py` (new): handler discovery/search/load/permission/formatting tests.
- `services/ai/tests/integration/test_model_prompt_and_tools.py` or `services/ai/tests/integration/test_dynamic_sources.py`: registration and end-to-end tool availability tests.

Possible files if implementation needs shared helpers rather than a standalone handler:
- `services/ai/tools/connector_handler.py`: factor/reuse connector manifest + active-source discovery for actions/resources/prompts.
- `services/searcher/tests/integration_tests.rs`: add generic coverage for `resource` and `prompt` capability types if current tests are too tool-specific.

Likely no migration change needed:
- `services/migrations/099_create_agent_capabilities.sql` already documents `prompt`/`resource` as future capability types and uses `TEXT` for `capability_type`.

## Reuse

- `services/ai/tools/meta_handler.py`: capability upsert batching/fingerprinting, searcher filtering with allowed IDs/source IDs, and exact-load UX.
- `services/ai/tools/skill_handler.py`: non-callable capability search/load UX and connector-backed skill loading pattern.
- `services/ai/tools/connector_handler.py`: connector manifest/source discovery, active/deleted source filtering, `source_filter` handling for agent contexts, and stable source metadata.
- `services/ai/tools/document_handler.py`: `start_line`/`end_line` schema and text line slicing behavior for large reads.
- `services/searcher/src/capabilities_repository.rs`: generic capability persistence/search with `allowed_ids` and `allowed_source_ids`.
- `services/connector-manager/src/handlers.rs`: existing `/resource` and `/prompt` endpoints that resolve source credentials and call connector SDKs.
- SDK adapter response shapes in `sdk/python/omni_connector/mcp_adapter.py`, `sdk/typescript/src/mcp-adapter.ts`, and `sdk/rust/src/mcp_adapter.rs`.

## Steps

- [ ] Create an MCP capability handler that fetches healthy connector manifests and active sources, then builds per-source resource and prompt records only for sources allowed in the current chat/agent context.
- [ ] Publish resource capabilities with stable IDs such as `resource:{source_id}:{hash(uri_template)}`, `capability_type="resource"`, searchable name/description/source text, and data containing `source_id`, `source_type`, `source_name`, `uri_template`, `name`, `description`, and `mime_type`.
- [ ] Publish prompt capabilities with stable IDs such as `prompt:{source_id}:{prompt_name}`, `capability_type="prompt"`, searchable name/description/argument text, and data containing `source_id`, `source_type`, `source_name`, `name`, `description`, and `arguments`.
- [ ] Implement `resource_search(query, limit)` and `prompt_search(query, limit)` using `search_capabilities` with `allowed_ids`/`allowed_source_ids` derived from the handler's accessible records.
- [ ] Implement `load_resource(resource_id, uri?, start_line?, end_line?)`: validate the selected resource belongs to the accessible records, require `uri` when the manifest entry is a URI template, call connector-manager `/resource`, and format text resources with line metadata and range slicing.
- [ ] Implement large-resource behavior: return inline only under a conservative size threshold or when a line range is supplied; otherwise return a preview, total line count, and instructions to reload with `start_line`/`end_line`.
- [ ] Implement `load_prompt(prompt_id, arguments?)`: validate required arguments from manifest metadata, call connector-manager `/prompt`, and return a structured tool result containing prompt provenance, description, and the returned prompt messages with roles/order preserved. Include explicit guidance that these are prompt-template messages, not actual chat history.
- [ ] Register the new handler in both `_build_registry` and `_build_agent_chat_registry` in `services/ai/routers/chat.py`; for user agents, apply the same `source_filter` used for read-only agent permissions.
- [ ] Update `services/ai/prompts.py` to tell the model to search/load MCP resources for source-provided reference data and MCP prompts for connector-provided workflows/templates, without preloading all content.
- [ ] Add tests for publishing, search, line-range loading, large-resource preview behavior, prompt loading/argument validation, structured prompt tool-result formatting, and permission filtering.

## Verification

- Run targeted unit tests, e.g. `pytest services/ai/tests/unit/test_mcp_capability_handler.py`.
- Run existing related handler tests, e.g. `pytest services/ai/tests/unit/test_meta_handler.py services/ai/tests/unit/test_skill_handler.py`.
- Run registration/integration tests touched by the change, e.g. `pytest services/ai/tests/integration/test_model_prompt_and_tools.py services/ai/tests/integration/test_dynamic_sources.py`.
- If adding searcher integration coverage, run `cargo test -p omni-searcher --test integration_tests` (confirm package name before execution).
- Manually verify a connector manifest resource and prompt can be found via search and loaded, while connectors exposing only MCP tools still register and work normally.

## Decisions

- MCP prompt messages will not be inserted as mainline chat messages. They will be returned as structured, provenance-marked tool results so the model can apply the template without confusing template roles with actual user/assistant history.
- For URI-template MCP resources, `resource_search` will return the template and `load_resource` will require a concrete `uri`, validating the chosen resource/source before reading it.
