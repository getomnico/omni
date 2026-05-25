-- Generic searchable key-value store for agent capabilities (tools, skills,
-- prompts, and future discoverable affordances). Searcher owns this table and
-- its ParadeDB index.

CREATE TABLE IF NOT EXISTS agent_capabilities (
    id VARCHAR(255) PRIMARY KEY,
    capability_type TEXT NOT NULL,
    user_id TEXT,
    search_text TEXT NOT NULL,
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_user_id
    ON agent_capabilities(user_id)
    WHERE user_id IS NOT NULL;

CREATE INDEX agent_capabilities_search_idx ON agent_capabilities
USING bm25 (
    id,
    (capability_type::pdb.literal),
    (user_id::pdb.literal),
    (search_text::pdb.simple('ascii_folding=true')),
    data
)
WITH (key_field = 'id');

COMMENT ON TABLE agent_capabilities IS
    'Searcher-owned generic key-value store for searchable agent capabilities.';
