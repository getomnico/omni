-- Projects: a user-owned organizational layer that groups chats, standing
-- instructions, and context attachments. Personal scope only (no sharing):
-- all access is mediated by projects.user_id, so projects never widen source
-- permissions beyond what the owning user already has.

CREATE TABLE projects (
    id CHAR(26) PRIMARY KEY,
    user_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    instructions TEXT,
    is_archived BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Names are unique per user among live projects (case-insensitive).
CREATE UNIQUE INDEX idx_projects_user_name ON projects(user_id, lower(name)) WHERE is_deleted = FALSE;

CREATE INDEX idx_projects_user_active ON projects(user_id, updated_at DESC) WHERE is_deleted = FALSE;

CREATE TRIGGER update_projects_updated_at
    BEFORE UPDATE ON projects
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Chat association. ON DELETE SET NULL keeps chat rows (and their usage
-- history) intact when a project is hard-deleted; the soft-delete path also
-- nulls the column explicitly so chats return to "unorganized".
ALTER TABLE chats ADD COLUMN project_id CHAR(26) REFERENCES projects(id) ON DELETE SET NULL;

CREATE INDEX idx_chats_project ON chats(project_id, updated_at DESC) WHERE is_deleted = FALSE;

-- Standing context attachments for a project. Two kinds:
--   upload:   a row in `uploads` (user-uploaded file), always owned by the
--             project owner, cascaded on upload removal.
--   document: a reference to a document in the unified index. Stored as TEXT
--             without an FK: connector re-syncs can replace document rows, and
--             a dangling reference must not cascade into project data.
CREATE TABLE project_attachments (
    id CHAR(26) PRIMARY KEY,
    project_id CHAR(26) NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    attachment_type TEXT NOT NULL CHECK (attachment_type IN ('upload', 'document')),
    upload_id CHAR(26) REFERENCES uploads(id) ON DELETE CASCADE,
    document_id TEXT,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT project_attachments_shape_check CHECK (
        (attachment_type = 'upload' AND upload_id IS NOT NULL AND document_id IS NULL)
        OR (attachment_type = 'document' AND document_id IS NOT NULL AND upload_id IS NULL)
    ),
    UNIQUE (project_id, upload_id),
    UNIQUE (project_id, document_id)
);

CREATE INDEX idx_project_attachments_project ON project_attachments(project_id);
