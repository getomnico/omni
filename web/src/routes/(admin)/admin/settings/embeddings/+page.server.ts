import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getEmbeddingConfig,
    upsertEmbeddingConfig,
    type EmbeddingProvider,
    type EmbeddingConfig,
} from '$lib/server/db/embedding-config'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const config = await getEmbeddingConfig()

    if (!config) {
        return { config: null, hasApiKey: false }
    }

    return {
        config: {
            provider: config.provider,
            model: config.model,
            apiUrl: config.apiUrl,
            dimensions: config.dimensions,
        },
        hasApiKey: !!config.apiKey,
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const provider = formData.get('provider') as EmbeddingProvider

        if (!provider) {
            return fail(400, { error: 'Provider is required', provider })
        }

        if (!['local', 'jina', 'openai', 'cohere', 'bedrock'].includes(provider)) {
            return fail(400, { error: 'Invalid provider', provider })
        }

        try {
            const existingConfig = await getEmbeddingConfig()

            const model = formData.get('model') as string
            const apiKey = (formData.get('apiKey') as string) || null
            const apiUrl = (formData.get('apiUrl') as string) || null
            const dimensionsStr = formData.get('dimensions') as string
            const dimensions = dimensionsStr ? parseInt(dimensionsStr, 10) : null

            if (!model) {
                return fail(400, { error: 'Model is required', provider })
            }

            // Validate API key for providers that require it
            if (['jina', 'openai', 'cohere'].includes(provider)) {
                const existingApiKey =
                    existingConfig?.provider === provider ? existingConfig.apiKey : null
                if (!apiKey && !existingApiKey) {
                    return fail(400, { error: 'API key is required for this provider', provider })
                }
            }

            // Validate API URL for local provider
            if (provider === 'local' && !apiUrl) {
                return fail(400, { error: 'API URL is required for Local provider', provider })
            }

            // Preserve existing API key if not provided
            const existingApiKey =
                existingConfig?.provider === provider ? existingConfig.apiKey : null
            const finalApiKey = apiKey || existingApiKey

            const configData: EmbeddingConfig = {
                provider,
                model,
                apiKey: finalApiKey,
                apiUrl,
                dimensions,
            }

            await upsertEmbeddingConfig(configData)

            return {
                success: true,
                message: 'Embedding configuration saved successfully',
            }
        } catch (err) {
            console.error('Failed to save embedding configuration:', err)
            return fail(500, { error: 'Failed to save embedding configuration' })
        }
    },
}
