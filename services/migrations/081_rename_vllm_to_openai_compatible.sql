ALTER TABLE model_providers DROP CONSTRAINT IF EXISTS model_providers_provider_type_check;
UPDATE model_providers SET provider_type = 'openai_compatible' WHERE provider_type = 'vllm';
ALTER TABLE model_providers ADD CONSTRAINT model_providers_provider_type_check
    CHECK (provider_type IN ('openai_compatible', 'anthropic', 'bedrock', 'openai', 'gemini', 'azure_foundry', 'vertex_ai'));
