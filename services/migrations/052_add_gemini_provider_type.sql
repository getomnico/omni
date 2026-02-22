ALTER TABLE model_providers DROP CONSTRAINT IF EXISTS model_providers_provider_type_check;
ALTER TABLE model_providers ADD CONSTRAINT model_providers_provider_type_check
    CHECK (provider_type IN ('vllm', 'anthropic', 'bedrock', 'openai', 'gemini'));
