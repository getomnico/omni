-- Per-key source scoping: null = all sources, ["gmail","slack"] = only those
ALTER TABLE api_keys ADD COLUMN allowed_sources JSONB DEFAULT NULL;

COMMENT ON COLUMN api_keys.allowed_sources IS 'JSON array of source_type strings this key can access. NULL = unrestricted.';
