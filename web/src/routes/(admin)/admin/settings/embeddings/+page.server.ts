import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    listActiveProviders,
    getProvider,
    getCurrentProvider,
    createProvider,
    updateProvider,
    deleteProvider,
    setCurrentProvider,
    EMBEDDING_PROVIDER_TYPES,
    PROVIDER_LABELS,
    type EmbeddingProviderConfig,
    type EmbeddingProviderType,
} from '$lib/server/db/embedding-providers'
import { env } from '$env/dynamic/private'

async function triggerReindex() {
    try {
        await fetch(`${env.INDEXER_URL}/admin/reindex-embeddings`, { method: 'POST' })
    } catch (err) {
        console.error('Failed to trigger re-indexing:', err)
    }
}

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const { apiKey, ...rest } = config
    return rest
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const providers = await listActiveProviders()

    return {
        providers: providers.map((p) => ({
            id: p.id,
            name: p.name,
            providerType: p.providerType,
            config: stripSecrets(p.config as Record<string, unknown>),
            hasApiKey: !!(p.config as Record<string, unknown>).apiKey,
            isCurrent: p.isCurrent,
        })),
    }
}

export const actions: Actions = {
    add: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const providerType = formData.get('providerType') as EmbeddingProviderType

        if (!providerType || !EMBEDDING_PROVIDER_TYPES.includes(providerType))
            return fail(400, { error: 'Invalid provider type' })

        const config = parseConfig(formData, providerType)
        const validation = validateConfig(providerType, config)
        if (validation) return fail(400, { error: validation })

        const name = PROVIDER_LABELS[providerType] || providerType

        try {
            const provider = await createProvider({ name, providerType, config })

            if (provider.isCurrent) {
                await triggerReindex()
            }

            return { success: true, message: 'Provider connected' }
        } catch (err) {
            console.error('Failed to add embedding provider:', err)
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

        const providerType = existing.providerType as EmbeddingProviderType
        const config = parseConfig(formData, providerType)

        // Preserve existing API key if not provided
        if (!config.apiKey) {
            const existingConfig = existing.config as Record<string, unknown>
            config.apiKey = (existingConfig.apiKey as string) || null
        }

        const validation = validateConfig(providerType, config, true)
        if (validation) return fail(400, { error: validation })

        try {
            await updateProvider(id, { config })

            return { success: true, message: 'Provider updated' }
        } catch (err) {
            console.error('Failed to update embedding provider:', err)
            return fail(500, { error: 'Failed to update provider' })
        }
    },

    delete: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        const existing = await getProvider(id)

        try {
            await deleteProvider(id)

            return { success: true, message: 'Provider removed' }
        } catch (err) {
            console.error('Failed to delete embedding provider:', err)
            return fail(500, { error: 'Failed to delete provider' })
        }
    },

    setCurrent: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })

        try {
            await setCurrentProvider(id)
            await triggerReindex()

            return { success: true, message: 'Embedding provider switched. Re-indexing started.' }
        } catch (err) {
            console.error('Failed to set current embedding provider:', err)
            return fail(500, { error: 'Failed to switch provider' })
        }
    },
}

function parseConfig(formData: FormData, providerType: string): EmbeddingProviderConfig {
    const model = (formData.get('model') as string)?.trim() || null
    const apiKey = (formData.get('apiKey') as string) || null
    const apiUrl = (formData.get('apiUrl') as string) || null
    const dimensionsStr = formData.get('dimensions') as string
    const dimensions = dimensionsStr ? parseInt(dimensionsStr, 10) : null
    const maxModelLenStr = formData.get('maxModelLen') as string
    const maxModelLen = maxModelLenStr ? parseInt(maxModelLenStr, 10) : null

    return { model, apiKey, apiUrl, dimensions, maxModelLen }
}

function validateConfig(
    providerType: string,
    config: EmbeddingProviderConfig,
    isEdit = false,
): string | null {
    if (!config.model) return 'Model is required'
    if (providerType === 'local' && !config.apiUrl) return 'API URL is required for Local provider'
    if (['jina', 'openai', 'cohere'].includes(providerType) && !config.apiKey && !isEdit)
        return 'API key is required for this provider'
    return null
}
