-- Permission scope for API keys: 'public' (only public docs) or 'user' (inherits user permissions)
ALTER TABLE api_keys ADD COLUMN scope TEXT NOT NULL DEFAULT 'public';

COMMENT ON COLUMN api_keys.scope IS 'Permission scope: "public" = only public documents, "user" = inherits creating user permissions';
