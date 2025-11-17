import { error, fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getEmbeddingConfig,
    upsertEmbeddingConfig,
    type EmbeddingProvider,
} from '$lib/server/db/embedding-config'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const config = await getEmbeddingConfig()

    return {
        config: config
            ? {
                  provider: config.provider,
                  jinaModel: config.jinaModel,
                  jinaApiUrl: config.jinaApiUrl,
                  bedrockModelId: config.bedrockModelId,
              }
            : null,
        hasJinaApiKey: config?.jinaApiKey ? true : false,
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()

        const provider = formData.get('provider') as EmbeddingProvider
        const jinaApiKey = (formData.get('jinaApiKey') as string) || null
        const jinaModel = (formData.get('jinaModel') as string) || null
        const jinaApiUrl = (formData.get('jinaApiUrl') as string) || null
        const bedrockModelId = (formData.get('bedrockModelId') as string) || null

        // Validation
        if (!provider) {
            return fail(400, {
                error: 'Provider is required',
                provider,
            })
        }

        if (!['jina', 'bedrock'].includes(provider)) {
            return fail(400, {
                error: 'Invalid provider',
                provider,
            })
        }

        // Provider-specific validation
        if (provider === 'jina' && !jinaModel) {
            return fail(400, {
                error: 'Jina model is required for Jina provider',
                provider,
            })
        }

        if (provider === 'bedrock' && !bedrockModelId) {
            return fail(400, {
                error: 'Bedrock model ID is required for Bedrock provider',
                provider,
            })
        }

        try {
            // Get existing config to preserve fields not being updated
            const existingConfig = await getEmbeddingConfig()

            await upsertEmbeddingConfig({
                provider,
                jinaApiKey: jinaApiKey || existingConfig?.jinaApiKey || null,
                jinaModel,
                jinaApiUrl,
                bedrockModelId,
            })

            return {
                success: true,
                message: 'Embedding configuration saved successfully',
            }
        } catch (err) {
            console.error('Failed to save embedding configuration:', err)
            return fail(500, {
                error: 'Failed to save embedding configuration',
            })
        }
    },
}
