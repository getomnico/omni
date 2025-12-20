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
        return {
            config: null,
            hasAnthropicApiKey: false,
        }
    }

    // Return provider-specific data based on config type
    switch (config.provider) {
        case 'vllm':
            return {
                config: {
                    provider: config.provider,
                    vllmUrl: config.vllmUrl,
                    primaryModelId: config.primaryModelId,
                    secondaryModelId: config.secondaryModelId,
                    maxTokens: config.maxTokens,
                    temperature: config.temperature,
                    topP: config.topP,
                },
                hasAnthropicApiKey: false,
            }
        case 'anthropic':
            return {
                config: {
                    provider: config.provider,
                    primaryModelId: config.primaryModelId,
                    secondaryModelId: config.secondaryModelId,
                    maxTokens: config.maxTokens,
                    temperature: config.temperature,
                    topP: config.topP,
                },
                hasAnthropicApiKey: !!config.anthropicApiKey,
            }
        case 'bedrock':
            return {
                config: {
                    provider: config.provider,
                    primaryModelId: config.primaryModelId,
                    secondaryModelId: config.secondaryModelId,
                    maxTokens: config.maxTokens,
                    temperature: config.temperature,
                    topP: config.topP,
                },
                hasAnthropicApiKey: false,
            }
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const provider = formData.get('provider') as LLMProvider

        // Validation
        if (!provider) {
            return fail(400, {
                error: 'Provider is required',
                provider,
            })
        }

        if (!['vllm', 'anthropic', 'bedrock'].includes(provider)) {
            return fail(400, {
                error: 'Invalid provider',
                provider,
            })
        }

        // Parse optional numeric fields
        const maxTokensStr = formData.get('maxTokens') as string
        const temperatureStr = formData.get('temperature') as string
        const topPStr = formData.get('topP') as string

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
            const existingConfig = await getLLMConfig()
            let configData: LLMConfig

            switch (provider) {
                case 'vllm': {
                    const vllmUrl = formData.get('vllmUrl') as string
                    const primaryModelId = (formData.get('primaryModelId') as string) || null
                    const secondaryModelId = (formData.get('secondaryModelId') as string) || null

                    if (!vllmUrl) {
                        return fail(400, {
                            error: 'vLLM URL is required for vLLM provider',
                            provider,
                        })
                    }

                    configData = {
                        provider: 'vllm',
                        vllmUrl,
                        primaryModelId,
                        secondaryModelId,
                        maxTokens,
                        temperature,
                        topP,
                    }
                    break
                }

                case 'anthropic': {
                    const primaryModelId = formData.get('primaryModelId') as string
                    const secondaryModelId = (formData.get('secondaryModelId') as string) || null
                    const anthropicApiKey = (formData.get('anthropicApiKey') as string) || null

                    if (!primaryModelId) {
                        return fail(400, {
                            error: 'Primary model ID is required for Anthropic provider',
                            provider,
                        })
                    }

                    // Preserve existing API key if not provided
                    const existingApiKey =
                        existingConfig?.provider === 'anthropic'
                            ? existingConfig.anthropicApiKey
                            : null

                    const finalApiKey = anthropicApiKey || existingApiKey
                    if (!finalApiKey) {
                        return fail(400, {
                            error: 'API key is required for Anthropic provider',
                            provider,
                        })
                    }

                    configData = {
                        provider: 'anthropic',
                        anthropicApiKey: finalApiKey,
                        primaryModelId,
                        secondaryModelId,
                        maxTokens,
                        temperature,
                        topP,
                    }
                    break
                }

                case 'bedrock': {
                    const primaryModelId = formData.get('primaryModelId') as string
                    const secondaryModelId = (formData.get('secondaryModelId') as string) || null

                    if (!primaryModelId) {
                        return fail(400, {
                            error: 'Primary model ID is required for Bedrock provider',
                            provider,
                        })
                    }

                    configData = {
                        provider: 'bedrock',
                        primaryModelId,
                        secondaryModelId,
                        maxTokens,
                        temperature,
                        topP,
                    }
                    break
                }

                default:
                    return fail(400, {
                        error: 'Invalid provider',
                        provider,
                    })
            }

            await upsertLLMConfig(configData)

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
