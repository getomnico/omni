-- User-created skill library entries only. Built-in and connector skills remain
-- sourced from static Markdown files and are indexed/cached separately.

CREATE TABLE IF NOT EXISTS skills (
    id CHAR(26) PRIMARY KEY,
    owner_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    instructions TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'private',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT skills_name_not_blank CHECK (btrim(name) <> ''),
    CONSTRAINT skills_description_not_blank CHECK (btrim(description) <> ''),
    CONSTRAINT skills_description_max_length CHECK (char_length(description) <= 500),
    CONSTRAINT skills_instructions_not_blank CHECK (btrim(instructions) <> ''),
    CONSTRAINT skills_visibility_check CHECK (visibility IN ('private', 'public'))
);

CREATE INDEX IF NOT EXISTS idx_skills_owner_updated
    ON skills (owner_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_skills_public_updated
    ON skills (updated_at DESC)
    WHERE visibility = 'public';

CREATE TRIGGER update_skills_updated_at BEFORE UPDATE ON skills
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE skills IS
    'User-created skill library entries only. Built-in and connector skills remain sourced from static Markdown files and are indexed/cached separately.';

ALTER TABLE agent_capabilities
    ADD COLUMN IF NOT EXISTS publisher_id TEXT;

CREATE INDEX IF NOT EXISTS idx_agent_capabilities_publisher_type
    ON agent_capabilities(publisher_id, capability_type);

COMMENT ON COLUMN agent_capabilities.publisher_id IS
    'Stable owner of this capability projection. Publishers sync their own capabilities atomically and prune stale rows.';
