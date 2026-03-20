-- People directory table
-- Schema aligned with Entra ID (Microsoft Graph user) and Okta (SCIM base profile)

-- ParadeDB does not support CHAR(n) columns for BM25 indexes, so use VARCHAR
CREATE TABLE people (
    id VARCHAR(26) PRIMARY KEY,
    email VARCHAR(255) UNIQUE NOT NULL,
    display_name VARCHAR(255),
    given_name VARCHAR(128),
    surname VARCHAR(128),
    avatar_url VARCHAR(500),

    -- Org context (will be populated by IDP connectors in the future)
    job_title VARCHAR(255),
    department VARCHAR(255),
    division VARCHAR(255),
    company_name VARCHAR(255),
    office_location VARCHAR(255),
    manager_id VARCHAR(26) REFERENCES people(id) ON DELETE SET NULL,

    -- Location
    city VARCHAR(128),
    state VARCHAR(128),
    country VARCHAR(128),

    -- Employee info
    employee_id VARCHAR(64),
    employee_type VARCHAR(64),
    cost_center VARCHAR(64),

    -- Status
    is_active BOOLEAN NOT NULL DEFAULT true,

    -- Extensibility
    metadata JSONB NOT NULL DEFAULT '{}',

    -- User ID in external employee directory
    external_id VARCHAR(255),

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- BM25 index for people search
CREATE INDEX people_search_idx ON people
USING bm25 (
    id,
    (email::pdb.simple('ascii_folding=true')),
    (display_name::pdb.simple('ascii_folding=true')),
    (given_name::pdb.simple('ascii_folding=true')),
    (surname::pdb.simple('ascii_folding=true')),
    (department::pdb.simple('ascii_folding=true')),
    (job_title::pdb.simple('ascii_folding=true')),
    (company_name::pdb.simple('ascii_folding=true'))
)
WITH (key_field = 'id');

-- B-tree indexes for exact lookups
CREATE INDEX idx_people_email ON people (lower(email));
CREATE INDEX idx_people_manager ON people (manager_id) WHERE manager_id IS NOT NULL;
