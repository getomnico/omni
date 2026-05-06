-- Track which connector toolsets a chat session has loaded so far.
--
-- Tools are no longer pushed wholesale into the LLM prompt; the model loads them on
-- demand via the `tool_search` / `load_tool_set` meta-tools (issue #203). This column
-- holds the source_ids whose actions have been admitted into the per-turn tool list,
-- so the discovery cost is paid once per (chat × source) instead of every turn.

ALTER TABLE chats ADD COLUMN loaded_toolsets TEXT[] NOT NULL DEFAULT '{}';

COMMENT ON COLUMN chats.loaded_toolsets IS
    'Source IDs whose connector actions have been loaded into this chat. Mutated by tool_search / load_tool_set meta-tools.';
