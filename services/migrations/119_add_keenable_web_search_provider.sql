-- Keenable web search provider.
ALTER TABLE web_search_providers DROP CONSTRAINT IF EXISTS web_search_providers_provider_type_check;
ALTER TABLE web_search_providers ADD CONSTRAINT web_search_providers_provider_type_check
CHECK (provider_type IN ('exa', 'serper', 'brave', 'keenable'));
