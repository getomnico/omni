-- Generic searchable key-value store for agent capabilities (tools, skills,
-- prompts, and future discoverable affordances). Searcher owns this table and
-- its ParadeDB index.

CREATE TABLE IF NOT EXISTS agent_capabilities (
    -- Stable id for replacement/upsert, namespaced by publisher, e.g. tool:gmail__send_email.
    id VARCHAR(255) PRIMARY KEY,
    -- Open-ended kind: tool, skill, prompt, resource, etc. Kept as TEXT instead of
    -- an enum so new capability kinds do not require DB migrations.
    capability_type TEXT NOT NULL,
    -- Stable model-facing/load name. For tools this is the callable tool name;
    -- for skills/prompts/resources this is the id the model should pass to load_*.
    name TEXT NOT NULL,
    -- Short display/selection text. Full bodies, schemas, and provider-specific
    -- payloads stay in data.
    description TEXT NOT NULL DEFAULT '',
    -- Nullable ownership/scope columns used for filtering. NULL means global/shared.
    user_id TEXT,
    source_id TEXT,
    source_type TEXT,
    -- Denormalized text deliberately chosen by the publisher for retrieval.
    -- Avoid indexing data::text, which makes arbitrary JSON shape part of ranking.
    search_text TEXT NOT NULL,
    -- Capability-specific payload needed to load/execute the capability.
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_type
    ON agent_capabilities(capability_type);

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_user_id
    ON agent_capabilities(user_id)
    WHERE user_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_source_id
    ON agent_capabilities(source_id)
    WHERE source_id IS NOT NULL;

CREATE INDEX agent_capabilities_search_idx ON agent_capabilities
USING bm25 (
    id,
    (capability_type::pdb.literal),
    (user_id::pdb.literal),
    (source_id::pdb.literal),
    (source_type::pdb.literal),
    (name::pdb.simple('ascii_folding=true')),
    (description::pdb.simple('ascii_folding=true')),
    (search_text::pdb.simple('ascii_folding=true'))
)
WITH (key_field = 'id');

COMMENT ON TABLE agent_capabilities IS
    'Searcher-owned catalog of discoverable agent capabilities: tools, skills, prompts, resources, and future loadable affordances.';
