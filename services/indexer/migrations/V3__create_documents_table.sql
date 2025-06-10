CREATE TABLE IF NOT EXISTS documents (
    id CHAR(26) PRIMARY KEY,
    source_id CHAR(26) NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    external_id VARCHAR(500) NOT NULL,
    title TEXT NOT NULL,
    content TEXT,
    content_type VARCHAR(100),
    file_size BIGINT,
    file_extension VARCHAR(50),
    url TEXT,
    parent_id VARCHAR(500),
    metadata JSONB NOT NULL DEFAULT '{}',
    permissions JSONB NOT NULL DEFAULT '[]',
    tsv_content tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', COALESCE(title, '')), 'A') ||
        setweight(to_tsvector('english', COALESCE(content, '')), 'B')
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(source_id, external_id)
);

CREATE INDEX idx_documents_source_id ON documents(source_id);
CREATE INDEX idx_documents_external_id ON documents(external_id);
CREATE INDEX idx_documents_content_type ON documents(content_type);
CREATE INDEX idx_documents_tsv_content ON documents USING GIN(tsv_content);
CREATE INDEX idx_documents_created_at ON documents(created_at);
CREATE INDEX idx_documents_updated_at ON documents(updated_at);

CREATE TRIGGER update_documents_updated_at BEFORE UPDATE ON documents
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();