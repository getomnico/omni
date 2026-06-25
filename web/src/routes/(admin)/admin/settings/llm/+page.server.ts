import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    listActiveProviders,
    getProvider,
    createProvider,
    updateProvider,
    deleteProvider,
    listModelsByProvider,
    createModel,
    deleteModel,
    setDefaultModel,
    setSecondaryModel,
    createModelSeeds,
    createPredefinedModels,
    MODEL_PROVIDER_TYPES,
    PREDEFINED_MODELS,
    type ModelProviderConfig,
    type ModelProviderType,
} from '$lib/server/db/model-providers'
import { env } from '$env/dynamic/private'
import { logger } from '$lib/server/logger'
import type {
    AvailableModel,
    ListProviderModelsResponse,
    TestModelRequest,
    TestModelResponse,
} from '$lib/types/model-provider'

async function reloadAIProviders() {
    try {
        await fetch(`${env.AI_SERVICE_URL}/admin/reload-providers`, { method: 'POST' })
    } catch (err) {
        console.error('Failed to reload AI providers:', err)
    }
}

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const { apiKey, ...rest } = config
    return rest
}

function rankDiscoveredModels(providerType: ModelProviderType, models: AvailableModel[]) {
    const predefined = PREDEFINED_MODELS[providerType] ?? []
    const discoveredById = new Map(models.map((m) => [m.model_id, m]))
    const preferred = predefined
        .filter((m) => discoveredById.has(m.modelId))
        .map((m) => ({ modelId: m.modelId, displayName: m.displayName }))
    const preferredIds = new Set(preferred.map((m) => m.modelId))
    const remaining = models
        .filter((m) => !preferredIds.has(m.model_id))
        .map((m) => ({ modelId: m.model_id, displayName: m.display_name }))
    return [...preferred, ...remaining].slice(0, 3)
}

async function listAvailableProviderModels(
    providerType: ModelProviderType,
    config: ModelProviderConfig,
) {
    const built = buildTestRequest(providerType, config, null)
    if ('error' in built) return []

    try {
        const resp = await fetch(`${env.AI_SERVICE_URL}/admin/provider/${providerType}/models`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(built),
        })
        if (!resp.ok) return []

        const body = (await resp.json()) as ListProviderModelsResponse
        return rankDiscoveredModels(providerType, body.models ?? [])
    } catch (err) {
        logger.warn('Failed to list provider models', { providerType, err })
        return []
    }
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const providers = await listActiveProviders()

    const providersWithModels = await Promise.all(
        providers.map(async (p) => {
            const providerModels = await listModelsByProvider(p.id)
            return {
                id: p.id,
                name: p.name,
                providerType: p.providerType,
                config: stripSecrets(p.config as Record<string, unknown>),
                hasApiKey: !!(p.config as Record<string, unknown>).apiKey,
                models: providerModels.map((m) => ({
                    id: m.id,
                    modelId: m.modelId,
                    displayName: m.displayName,
                    isDefault: m.isDefault,
                    isSecondary: m.isSecondary,
                })),
            }
        }),
    )

    return {
        providers: providersWithModels,
    }
}

export const actions: Actions = {
    add: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const name = (formData.get('name') as string)?.trim()
        const providerType = formData.get('providerType') as ModelProviderType

        if (!name) return fail(400, { error: 'Name is required' })
        if (!providerType || !MODEL_PROVIDER_TYPES.includes(providerType))
            return fail(400, { error: 'Invalid provider type' })

        const config = parseConfig(formData, providerType)
        const validation = validateConfig(providerType, config)
        if (validation) return fail(400, { error: validation })

        try {
            const provider = await createProvider({ name, providerType, config })
            const discoveredModels = await listAvailableProviderModels(providerType, config)
            if (discoveredModels.length > 0) {
                await createModelSeeds(provider.id, discoveredModels)
            } else {
                await createPredefinedModels(provider.id, providerType)
            }
            await reloadAIProviders()
            return { success: true, message: 'Provider connected' }
        } catch (err) {
            console.error('Failed to add provider:', err)
            return fail(500, { error: 'Failed to add provider' })
        }
    },

    edit: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        const existing = await getProvider(id)
        if (!existing) return fail(404, { error: 'Provider not found' })

        const name = (formData.get('name') as string)?.trim()
        const providerType = existing.providerType as ModelProviderType

        const config = parseConfig(formData, providerType)

        // Preserve existing API key if not provided
        if (!config.apiKey) {
            const existingConfig = existing.config as Record<string, unknown>
            config.apiKey = (existingConfig.apiKey as string) || null
        }

        const validation = validateConfig(providerType, config, true)
        if (validation) return fail(400, { error: validation })

        try {
            await updateProvider(id, { name, config })
            await reloadAIProviders()
            return { success: true, message: 'Provider updated' }
        } catch (err) {
            console.error('Failed to update provider:', err)
            return fail(500, { error: 'Failed to update provider' })
        }
    },

    delete: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        try {
            await deleteProvider(id)
            await reloadAIProviders()
            return { success: true, message: 'Provider deleted' }
        } catch (err) {
            console.error('Failed to delete provider:', err)
            return fail(500, { error: 'Failed to delete provider' })
        }
    },

    addModel: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const providerId = formData.get('providerId') as string
        const modelId = (formData.get('modelId') as string)?.trim()
        const displayName = (formData.get('displayName') as string)?.trim()
        const isDefault = formData.get('isDefault') === 'true'
        const isSecondary = formData.get('isSecondary') === 'true'

        if (!providerId) return fail(400, { error: 'Provider ID is required' })
        if (!modelId) return fail(400, { error: 'Model ID is required' })
        if (!displayName) return fail(400, { error: 'Display name is required' })

        try {
            await createModel({
                modelProviderId: providerId,
                modelId,
                displayName,
                isDefault,
                isSecondary,
            })
            await reloadAIProviders()
            return { success: true, message: 'Model added' }
        } catch (err) {
            console.error('Failed to add model:', err)
            return fail(500, { error: 'Failed to add model' })
        }
    },

    deleteModel: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Model ID is required' })

        try {
            await deleteModel(id)
            await reloadAIProviders()
            return { success: true, message: 'Model deleted' }
        } catch (err) {
            console.error('Failed to delete model:', err)
            return fail(500, { error: 'Failed to delete model' })
        }
    },

    setDefaultModel: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Model ID is required' })

        try {
            await setDefaultModel(id)
            return { success: true, message: 'Default model updated' }
        } catch (err) {
            console.error('Failed to set default model:', err)
            return fail(500, { error: 'Failed to set default model' })
        }
    },

    setSecondaryModel: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Model ID is required' })

        try {
            await setSecondaryModel(id)
            return { success: true, message: 'Secondary model updated' }
        } catch (err) {
            console.error('Failed to set secondary model:', err)
            return fail(500, { error: 'Failed to set secondary model' })
        }
    },

    testConnection: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = (formData.get('id') as string) || null
        const providerType = formData.get('providerType') as ModelProviderType

        if (!providerType || !MODEL_PROVIDER_TYPES.includes(providerType))
            return fail(400, { error: 'Invalid provider type' })

        const config = parseConfig(formData, providerType)
        let modelId = (formData.get('modelId') as string) || null

        if (id) {
            const existing = await getProvider(id)
            if (!existing) return fail(404, { error: 'Provider not found' })
            if (!config.apiKey) {
                config.apiKey = (existing.config as ModelProviderConfig).apiKey ?? null
            }
            if (!modelId) {
                const providerModels = await listModelsByProvider(id)
                modelId =
                    providerModels.find((m) => m.isDefault)?.modelId ??
                    providerModels[0]?.modelId ??
                    null
            }
        }

        const built = buildTestRequest(providerType, config, modelId)
        if ('error' in built) return fail(400, { error: built.error })

        try {
            const resp = await fetch(`${env.AI_SERVICE_URL}/admin/provider/${providerType}/test`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(built),
            })
            const body = (await resp.json()) as TestModelResponse
            if (body.ok) {
                const detail = [body.model, body.latency_ms ? `${body.latency_ms}ms` : null]
                    .filter((p): p is string => !!p)
                    .join(' · ')
                return {
                    success: true,
                    message: detail ? `Connected (${detail})` : 'Connected',
                }
            }
            return fail(400, {
                error: body.error || 'Connection failed',
                provider: body.provider,
                statusCode: body.status_code,
                model: body.model,
            })
        } catch (err) {
            logger.error('Model test connection failed', err)
            return fail(500, { error: 'Could not reach AI service' })
        }
    },
}

function buildTestRequest(
    providerType: ModelProviderType,
    config: ModelProviderConfig,
    modelId: string | null,
): TestModelRequest | { error: string } {
    switch (providerType) {
        case 'anthropic':
            if (!config.apiKey) return { error: 'API key is required for Anthropic' }
            return { api_key: config.apiKey, model: modelId }
        case 'openai':
            if (!config.apiKey) return { error: 'API key is required for OpenAI' }
            return { api_key: config.apiKey, model: modelId }
        case 'gemini':
            if (!config.apiKey) return { error: 'API key is required for Gemini' }
            return { api_key: config.apiKey, model: modelId }
        case 'bedrock':
            return {
                region_name: config.regionName ?? null,
                model_id: modelId,
            }
        case 'vertex_ai':
            if (!config.regionName) return { error: 'GCP Region is required for Vertex AI' }
            if (!config.projectId) return { error: 'GCP Project ID is required for Vertex AI' }
            return {
                region: config.regionName,
                project_id: config.projectId,
                model: modelId,
            }
        case 'azure_foundry':
            if (!config.apiUrl) return { error: 'Endpoint URL is required for Azure AI Foundry' }
            return {
                endpoint_url: config.apiUrl,
                model: modelId,
            }
        case 'openai_compatible':
            if (!config.apiUrl)
                return { error: 'Base URL is required for OpenAI-compatible provider' }
            return {
                base_url: config.apiUrl,
                api_key: config.apiKey ?? null,
                model: modelId,
            }
    }
}

function parseConfig(formData: FormData, providerType: string): ModelProviderConfig {
    return {
        apiKey: (formData.get('apiKey') as string) || null,
        apiUrl: (formData.get('apiUrl') as string) || null,
        regionName: (formData.get('regionName') as string) || null,
        projectId: (formData.get('projectId') as string) || null,
    }
}

function validateConfig(
    providerType: string,
    config: ModelProviderConfig,
    isEdit = false,
): string | null {
    if (providerType === 'azure_foundry' && !config.apiUrl)
        return 'Endpoint URL is required for Azure AI Foundry'
    if (providerType === 'openai_compatible' && !config.apiUrl)
        return 'Base URL is required for OpenAI-compatible provider'
    if (providerType === 'anthropic' && !config.apiKey && !isEdit)
        return 'API key is required for Anthropic'
    if (providerType === 'openai' && !config.apiKey && !isEdit)
        return 'API key is required for OpenAI'
    if (providerType === 'gemini' && !config.apiKey && !isEdit)
        return 'API key is required for Gemini'
    if (providerType === 'vertex_ai' && !config.regionName)
        return 'GCP Region is required for Vertex AI'
    if (providerType === 'vertex_ai' && !config.projectId)
        return 'GCP Project ID is required for Vertex AI'

    return null
}
