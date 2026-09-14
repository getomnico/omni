-- Per-chat connector source deselection. Sources listed here are excluded from
-- the chat: their toolsets are not advertised to the agent and cannot be loaded
-- via tool_search / load_tool / load_tool_set. Empty by default (all sources).

ALTER TABLE chats ADD COLUMN excluded_source_ids TEXT[] NOT NULL DEFAULT '{}';
