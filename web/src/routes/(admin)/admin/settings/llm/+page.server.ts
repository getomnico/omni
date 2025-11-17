import { error, fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getLLMConfig, upsertLLMConfig, type LLMProvider } from '$lib/server/db/llm-config'
import { env } from '$env/dynamic/private'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const config = await getLLMConfig()

    return {
        config: config
            ? {
                  provider: config.provider,
                  primaryModelId: config.primaryModelId,
                  secondaryModelId: config.secondaryModelId,
                  vllmUrl: config.vllmUrl,
                  maxTokens: config.maxTokens,
                  temperature: config.temperature,
                  topP: config.topP,
              }
            : null,
        hasAnthropicApiKey: config?.anthropicApiKey ? true : false,
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()

        const provider = formData.get('provider') as LLMProvider
        const primaryModelId = formData.get('primaryModelId') as string
        const secondaryModelId = (formData.get('secondaryModelId') as string) || null
        const vllmUrl = (formData.get('vllmUrl') as string) || null
        const anthropicApiKey = (formData.get('anthropicApiKey') as string) || null
        const maxTokensStr = formData.get('maxTokens') as string
        const temperatureStr = formData.get('temperature') as string
        const topPStr = formData.get('topP') as string

        // Validation
        if (!provider || !primaryModelId) {
            return fail(400, {
                error: 'Provider and primary model ID are required',
                provider,
                primaryModelId,
            })
        }

        if (!['vllm', 'anthropic', 'bedrock'].includes(provider)) {
            return fail(400, {
                error: 'Invalid provider',
                provider,
            })
        }

        // Provider-specific validation
        if (provider === 'vllm' && !vllmUrl) {
            return fail(400, {
                error: 'vLLM URL is required for vLLM provider',
                provider,
                vllmUrl,
            })
        }

        // Parse optional numeric fields
        const maxTokens = maxTokensStr ? parseInt(maxTokensStr) : null
        const temperature = temperatureStr ? parseFloat(temperatureStr) : null
        const topP = topPStr ? parseFloat(topPStr) : null

        // Validate numeric ranges
        if (temperature !== null && (temperature < 0 || temperature > 2)) {
            return fail(400, {
                error: 'Temperature must be between 0 and 2',
                temperature,
            })
        }

        if (topP !== null && (topP < 0 || topP > 1)) {
            return fail(400, {
                error: 'Top P must be between 0 and 1',
                topP,
            })
        }

        if (maxTokens !== null && maxTokens <= 0) {
            return fail(400, {
                error: 'Max tokens must be greater than 0',
                maxTokens,
            })
        }

        try {
            // Get existing config to preserve fields not being updated
            const existingConfig = await getLLMConfig()

            await upsertLLMConfig({
                provider,
                primaryModelId,
                secondaryModelId,
                vllmUrl,
                anthropicApiKey: anthropicApiKey || existingConfig?.anthropicApiKey || null,
                maxTokens,
                temperature,
                topP,
            })

            return {
                success: true,
                message: 'LLM configuration saved successfully',
            }
        } catch (err) {
            console.error('Failed to save LLM configuration:', err)
            return fail(500, {
                error: 'Failed to save LLM configuration',
            })
        }
    },
}
