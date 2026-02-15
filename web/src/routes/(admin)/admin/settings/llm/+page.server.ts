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
    createPredefinedModels,
    MODEL_PROVIDER_TYPES,
    type ModelProviderConfig,
    type ModelProviderType,
} from '$lib/server/db/model-providers'
import { env } from '$env/dynamic/private'

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
            await createPredefinedModels(provider.id, providerType)
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

        if (!providerId) return fail(400, { error: 'Provider ID is required' })
        if (!modelId) return fail(400, { error: 'Model ID is required' })
        if (!displayName) return fail(400, { error: 'Display name is required' })

        try {
            await createModel({
                modelProviderId: providerId,
                modelId,
                displayName,
                isDefault,
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
            await reloadAIProviders()
            return { success: true, message: 'Default model updated' }
        } catch (err) {
            console.error('Failed to set default model:', err)
            return fail(500, { error: 'Failed to set default model' })
        }
    },
}

function parseConfig(formData: FormData, providerType: string): ModelProviderConfig {
    return {
        apiKey: (formData.get('apiKey') as string) || null,
        apiUrl: (formData.get('apiUrl') as string) || null,
        regionName: (formData.get('regionName') as string) || null,
    }
}

function validateConfig(
    providerType: string,
    config: ModelProviderConfig,
    isEdit = false,
): string | null {
    if (providerType === 'vllm' && !config.apiUrl) return 'API URL is required for vLLM'
    if (providerType === 'anthropic' && !config.apiKey && !isEdit)
        return 'API key is required for Anthropic'
    if (providerType === 'openai' && !config.apiKey && !isEdit)
        return 'API key is required for OpenAI'

    return null
}
