CREATE TABLE web_search_providers (
    id CHAR(26) PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('exa', 'serper', 'brave')),
    config JSONB NOT NULL DEFAULT '{}',
    is_current BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_web_search_providers_single_current
    ON web_search_providers (is_current) WHERE is_current = TRUE AND is_deleted = FALSE;

CREATE TRIGGER set_updated_at BEFORE UPDATE ON web_search_providers
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TABLE web_fetch_providers (
    id CHAR(26) PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('exa', 'firecrawl')),
    config JSONB NOT NULL DEFAULT '{}',
    is_current BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_web_fetch_providers_single_current
    ON web_fetch_providers (is_current) WHERE is_current = TRUE AND is_deleted = FALSE;

CREATE TRIGGER set_updated_at BEFORE UPDATE ON web_fetch_providers
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE OR REPLACE FUNCTION notify_web_search_provider_change() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('web_search_provider_changed', json_build_object(
        'id', NEW.id,
        'is_current', NEW.is_current,
        'is_deleted', NEW.is_deleted
    )::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER web_search_provider_notify
    AFTER INSERT OR UPDATE ON web_search_providers
    FOR EACH ROW
    EXECUTE FUNCTION notify_web_search_provider_change();

CREATE OR REPLACE FUNCTION notify_web_fetch_provider_change() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('web_fetch_provider_changed', json_build_object(
        'id', NEW.id,
        'is_current', NEW.is_current,
        'is_deleted', NEW.is_deleted
    )::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER web_fetch_provider_notify
    AFTER INSERT OR UPDATE ON web_fetch_providers
    FOR EACH ROW
    EXECUTE FUNCTION notify_web_fetch_provider_change();

INSERT INTO configuration (scope, user_id, key, value)
VALUES ('global', NULL, 'web_access_policy', '{"blocklist": []}')
ON CONFLICT DO NOTHING;
