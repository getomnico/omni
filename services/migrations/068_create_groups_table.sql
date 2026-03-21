CREATE TABLE groups (
    id VARCHAR(26) PRIMARY KEY,
    source_id VARCHAR(26) NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    email VARCHAR(255) NOT NULL,
    display_name VARCHAR(255),
    description TEXT,
    synced_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (source_id, email)
);

CREATE INDEX idx_groups_source ON groups (source_id);
CREATE INDEX idx_groups_email ON groups (lower(email));

CREATE TABLE group_memberships (
    id VARCHAR(26) PRIMARY KEY,
    group_id VARCHAR(26) NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    member_email VARCHAR(255) NOT NULL,
    role VARCHAR(20),
    synced_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (group_id, member_email)
);

CREATE INDEX idx_group_memberships_member ON group_memberships (lower(member_email));
CREATE INDEX idx_group_memberships_group ON group_memberships (group_id);
