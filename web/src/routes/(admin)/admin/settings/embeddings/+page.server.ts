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
        return {
            config: null,
            hasJinaApiKey: false,
            hasOpenaiApiKey: false,
        }
    }

    // Return provider-specific data based on config type
    switch (config.provider) {
        case 'local':
            return {
                config: {
                    provider: config.provider,
                    localBaseUrl: config.localBaseUrl,
                    localModel: config.localModel,
                },
                hasJinaApiKey: false,
                hasOpenaiApiKey: false,
            }
        case 'jina':
            return {
                config: {
                    provider: config.provider,
                    jinaModel: config.jinaModel,
                    jinaApiUrl: config.jinaApiUrl,
                },
                hasJinaApiKey: !!config.jinaApiKey,
                hasOpenaiApiKey: false,
            }
        case 'openai':
            return {
                config: {
                    provider: config.provider,
                    openaiModel: config.openaiModel,
                    openaiDimensions: config.openaiDimensions,
                },
                hasJinaApiKey: false,
                hasOpenaiApiKey: !!config.openaiApiKey,
            }
        case 'bedrock':
            return {
                config: {
                    provider: config.provider,
                    bedrockModelId: config.bedrockModelId,
                },
                hasJinaApiKey: false,
                hasOpenaiApiKey: false,
            }
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const provider = formData.get('provider') as EmbeddingProvider

        // Validation
        if (!provider) {
            return fail(400, {
                error: 'Provider is required',
                provider,
            })
        }

        if (!['local', 'jina', 'openai', 'bedrock'].includes(provider)) {
            return fail(400, {
                error: 'Invalid provider',
                provider,
            })
        }

        try {
            const existingConfig = await getEmbeddingConfig()
            let configData: EmbeddingConfig

            switch (provider) {
                case 'local': {
                    const localBaseUrl = formData.get('localBaseUrl') as string
                    const localModel = formData.get('localModel') as string

                    if (!localBaseUrl) {
                        return fail(400, {
                            error: 'Base URL is required for Local provider',
                            provider,
                        })
                    }
                    if (!localModel) {
                        return fail(400, {
                            error: 'Model is required for Local provider',
                            provider,
                        })
                    }

                    configData = {
                        provider: 'local',
                        localBaseUrl,
                        localModel,
                    }
                    break
                }

                case 'jina': {
                    const jinaModel = formData.get('jinaModel') as string
                    const jinaApiKey = (formData.get('jinaApiKey') as string) || null
                    const jinaApiUrl = (formData.get('jinaApiUrl') as string) || null

                    if (!jinaModel) {
                        return fail(400, {
                            error: 'Model is required for Jina provider',
                            provider,
                        })
                    }

                    // Preserve existing API key if not provided
                    const existingApiKey =
                        existingConfig?.provider === 'jina' ? existingConfig.jinaApiKey : null

                    configData = {
                        provider: 'jina',
                        jinaModel,
                        jinaApiKey: jinaApiKey || existingApiKey,
                        jinaApiUrl,
                    }
                    break
                }

                case 'openai': {
                    const openaiModel = formData.get('openaiModel') as string
                    const openaiApiKey = (formData.get('openaiApiKey') as string) || null
                    const openaiDimensionsStr = formData.get('openaiDimensions') as string
                    const openaiDimensions = openaiDimensionsStr
                        ? parseInt(openaiDimensionsStr, 10)
                        : null

                    if (!openaiModel) {
                        return fail(400, {
                            error: 'Model is required for OpenAI provider',
                            provider,
                        })
                    }

                    // Preserve existing API key if not provided
                    const existingApiKey =
                        existingConfig?.provider === 'openai' ? existingConfig.openaiApiKey : null

                    configData = {
                        provider: 'openai',
                        openaiModel,
                        openaiApiKey: openaiApiKey || existingApiKey,
                        openaiDimensions,
                    }
                    break
                }

                case 'bedrock': {
                    const bedrockModelId = formData.get('bedrockModelId') as string

                    if (!bedrockModelId) {
                        return fail(400, {
                            error: 'Model ID is required for Bedrock provider',
                            provider,
                        })
                    }

                    configData = {
                        provider: 'bedrock',
                        bedrockModelId,
                    }
                    break
                }

                default:
                    return fail(400, {
                        error: 'Invalid provider',
                        provider,
                    })
            }

            await upsertEmbeddingConfig(configData)

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
