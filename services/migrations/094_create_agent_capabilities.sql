-- Searchable projection of agent capabilities (tools, skills, and future
-- discoverable affordances). Searcher owns this table and its ParadeDB index;
-- canonical state remains with the publishing service.

CREATE TABLE IF NOT EXISTS agent_capabilities (
    capability_id VARCHAR(255) PRIMARY KEY,
    capability_type VARCHAR(50) NOT NULL,
    item_id TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    source_id TEXT,
    source_type TEXT,
    visibility JSONB NOT NULL DEFAULT '{}',
    metadata JSONB NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_type
    ON agent_capabilities(capability_type);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_item_id
    ON agent_capabilities(item_id);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_source_id
    ON agent_capabilities(source_id)
    WHERE source_id IS NOT NULL;

CREATE INDEX agent_capabilities_search_idx ON agent_capabilities
USING bm25 (
    capability_id,
    (capability_type::pdb.literal),
    (item_id::pdb.simple('ascii_folding=true')),
    (title::pdb.simple('ascii_folding=true')),
    (description::pdb.simple('ascii_folding=true')),
    (body::pdb.simple('ascii_folding=true')),
    (source_id::pdb.literal),
    (source_type::pdb.simple('ascii_folding=true')),
    metadata,
    visibility
)
WITH (key_field = 'capability_id');

COMMENT ON TABLE agent_capabilities IS
    'Searcher-owned BM25 projection of agent-usable capabilities such as tools and skills.';
