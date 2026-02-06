import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getLLMConfig,
    upsertLLMConfig,
    type LLMProvider,
    type LLMConfig,
} from '$lib/server/db/llm-config'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const config = await getLLMConfig()

    if (!config) {
        return { config: null, hasApiKey: false }
    }

    return {
        config: {
            provider: config.provider,
            model: config.model,
            apiUrl: config.apiUrl,
            secondaryModel: config.secondaryModel,
            maxTokens: config.maxTokens,
            temperature: config.temperature,
            topP: config.topP,
        },
        hasApiKey: !!config.apiKey,
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const provider = formData.get('provider') as LLMProvider

        if (!provider) {
            return fail(400, { error: 'Provider is required', provider })
        }

        if (!['vllm', 'anthropic', 'bedrock'].includes(provider)) {
            return fail(400, { error: 'Invalid provider', provider })
        }

        // Parse optional numeric fields
        const maxTokensStr = formData.get('maxTokens') as string
        const temperatureStr = formData.get('temperature') as string
        const topPStr = formData.get('topP') as string

        const maxTokens = maxTokensStr ? parseInt(maxTokensStr) : null
        const temperature = temperatureStr ? parseFloat(temperatureStr) : null
        const topP = topPStr ? parseFloat(topPStr) : null

        if (temperature !== null && (temperature < 0 || temperature > 2)) {
            return fail(400, { error: 'Temperature must be between 0 and 2', temperature })
        }

        if (topP !== null && (topP < 0 || topP > 1)) {
            return fail(400, { error: 'Top P must be between 0 and 1', topP })
        }

        if (maxTokens !== null && maxTokens <= 0) {
            return fail(400, { error: 'Max tokens must be greater than 0', maxTokens })
        }

        try {
            const existingConfig = await getLLMConfig()

            const model = (formData.get('model') as string) || ''
            const apiKey = (formData.get('apiKey') as string) || null
            const apiUrl = (formData.get('apiUrl') as string) || null
            const secondaryModel = (formData.get('secondaryModel') as string) || null

            // Validate model for non-vllm providers
            if (provider !== 'vllm' && !model) {
                return fail(400, { error: 'Model is required for this provider', provider })
            }

            // Validate API URL for vllm
            if (provider === 'vllm' && !apiUrl) {
                return fail(400, { error: 'API URL is required for vLLM provider', provider })
            }

            // Validate API key for anthropic
            if (provider === 'anthropic') {
                const existingApiKey =
                    existingConfig?.provider === 'anthropic' ? existingConfig.apiKey : null
                if (!apiKey && !existingApiKey) {
                    return fail(400, {
                        error: 'API key is required for Anthropic provider',
                        provider,
                    })
                }
            }

            // Preserve existing API key if not provided
            const existingApiKey =
                existingConfig?.provider === provider ? existingConfig.apiKey : null
            const finalApiKey = apiKey || existingApiKey

            const configData: LLMConfig = {
                provider,
                model,
                apiKey: finalApiKey,
                apiUrl,
                secondaryModel,
                maxTokens,
                temperature,
                topP,
            }

            await upsertLLMConfig(configData)

            return {
                success: true,
                message: 'LLM configuration saved successfully',
            }
        } catch (err) {
            console.error('Failed to save LLM configuration:', err)
            return fail(500, { error: 'Failed to save LLM configuration' })
        }
    },
}
