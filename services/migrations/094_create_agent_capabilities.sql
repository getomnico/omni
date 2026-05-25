-- Generic searchable key-value store for agent capabilities (tools, skills,
-- prompts, and future discoverable affordances). Searcher owns this table and
-- its ParadeDB index.

CREATE TABLE IF NOT EXISTS agent_capabilities (
    id VARCHAR(255) PRIMARY KEY,
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX agent_capabilities_search_idx ON agent_capabilities
USING bm25 (
    id,
    data
)
WITH (key_field = 'id');

COMMENT ON TABLE agent_capabilities IS
    'Searcher-owned generic key-value store for searchable agent capabilities.';
