import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    listActiveProviders as listSearchProviders,
    getProvider as getSearchProvider,
    createProvider as createSearchProvider,
    updateProvider as updateSearchProvider,
    deleteProvider as deleteSearchProvider,
    setCurrentProvider as setCurrentSearchProvider,
    WEB_SEARCH_PROVIDER_TYPES,
    WEB_SEARCH_PROVIDER_LABELS,
    type WebSearchProviderConfig,
    type WebSearchProviderType,
} from '$lib/server/db/web-search-providers'
import {
    listActiveProviders as listFetchProviders,
    getProvider as getFetchProvider,
    createProvider as createFetchProvider,
    updateProvider as updateFetchProvider,
    deleteProvider as deleteFetchProvider,
    setCurrentProvider as setCurrentFetchProvider,
    WEB_FETCH_PROVIDER_TYPES,
    WEB_FETCH_PROVIDER_LABELS,
    type WebFetchProviderConfig,
    type WebFetchProviderType,
} from '$lib/server/db/web-fetch-providers'
import { getGlobal, setGlobal } from '$lib/server/db/configuration'

const WEB_ACCESS_POLICY_KEY = 'web_access_policy'

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const { apiKey, ...rest } = config
    return rest
}

function parseSearchConfig(formData: FormData): WebSearchProviderConfig {
    return {
        apiKey: ((formData.get('apiKey') as string) || '').trim() || null,
        baseUrl: ((formData.get('baseUrl') as string) || '').trim() || null,
    }
}

function parseFetchConfig(formData: FormData): WebFetchProviderConfig {
    return {
        apiKey: ((formData.get('apiKey') as string) || '').trim() || null,
        baseUrl: ((formData.get('baseUrl') as string) || '').trim() || null,
    }
}

function validateConfig(config: { apiKey?: string | null }, isEdit = false): string | null {
    if (!config.apiKey && !isEdit) return 'API key is required'
    return null
}

function parseBlocklist(value: string): string[] {
    return value
        .split('\n')
        .map((line) => line.trim().toLowerCase())
        .filter(Boolean)
}

function validateBlocklist(patterns: string[]): string | null {
    for (const pattern of patterns) {
        if (pattern.includes('*') && !pattern.startsWith('*.')) {
            return `Invalid blocklist pattern "${pattern}". Wildcards are only supported as *.example.com.`
        }
        if (pattern.includes('://')) {
            if (!pattern.startsWith('http://') && !pattern.startsWith('https://')) {
                return `Invalid URL prefix "${pattern}". Only http:// and https:// prefixes are supported.`
            }
        }
    }
    return null
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const [searchProviders, fetchProviders, policy] = await Promise.all([
        listSearchProviders(),
        listFetchProviders(),
        getGlobal(WEB_ACCESS_POLICY_KEY),
    ])
    const blocklist = Array.isArray(policy?.blocklist) ? (policy.blocklist as string[]) : []

    return {
        searchProviders: searchProviders.map((p) => ({
            id: p.id,
            name: p.name,
            providerType: p.providerType,
            config: stripSecrets(p.config as Record<string, unknown>),
            hasApiKey: !!(p.config as Record<string, unknown>).apiKey,
            isCurrent: p.isCurrent,
        })),
        fetchProviders: fetchProviders.map((p) => ({
            id: p.id,
            name: p.name,
            providerType: p.providerType,
            config: stripSecrets(p.config as Record<string, unknown>),
            hasApiKey: !!(p.config as Record<string, unknown>).apiKey,
            isCurrent: p.isCurrent,
        })),
        blocklist,
    }
}

export const actions: Actions = {
    addSearchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const providerType = formData.get('providerType') as WebSearchProviderType
        if (!providerType || !WEB_SEARCH_PROVIDER_TYPES.includes(providerType)) {
            return fail(400, { error: 'Invalid web search provider type' })
        }
        const config = parseSearchConfig(formData)
        const validation = validateConfig(config)
        if (validation) return fail(400, { error: validation })

        try {
            await createSearchProvider({
                name: WEB_SEARCH_PROVIDER_LABELS[providerType],
                providerType,
                config,
            })
            return { success: true, message: 'Web search provider connected' }
        } catch (err) {
            console.error('Failed to add web search provider:', err)
            return fail(500, { error: 'Failed to add web search provider' })
        }
    },

    editSearchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        const existing = await getSearchProvider(id)
        if (!existing) return fail(404, { error: 'Provider not found' })
        const config = parseSearchConfig(formData)
        if (!config.apiKey) config.apiKey = (existing.config as Record<string, string>).apiKey || null
        const validation = validateConfig(config, true)
        if (validation) return fail(400, { error: validation })

        try {
            await updateSearchProvider(id, { config })
            return { success: true, message: 'Web search provider updated' }
        } catch (err) {
            console.error('Failed to update web search provider:', err)
            return fail(500, { error: 'Failed to update web search provider' })
        }
    },

    deleteSearchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        await deleteSearchProvider(id)
        return { success: true, message: 'Web search provider removed' }
    },

    setCurrentSearchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        await setCurrentSearchProvider(id)
        return { success: true, message: 'Current web search provider updated' }
    },

    addFetchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const providerType = formData.get('providerType') as WebFetchProviderType
        if (!providerType || !WEB_FETCH_PROVIDER_TYPES.includes(providerType)) {
            return fail(400, { error: 'Invalid web fetch provider type' })
        }
        const config = parseFetchConfig(formData)
        const validation = validateConfig(config)
        if (validation) return fail(400, { error: validation })

        try {
            await createFetchProvider({
                name: WEB_FETCH_PROVIDER_LABELS[providerType],
                providerType,
                config,
            })
            return { success: true, message: 'Web fetch provider connected' }
        } catch (err) {
            console.error('Failed to add web fetch provider:', err)
            return fail(500, { error: 'Failed to add web fetch provider' })
        }
    },

    editFetchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        const existing = await getFetchProvider(id)
        if (!existing) return fail(404, { error: 'Provider not found' })
        const config = parseFetchConfig(formData)
        if (!config.apiKey) config.apiKey = (existing.config as Record<string, string>).apiKey || null
        const validation = validateConfig(config, true)
        if (validation) return fail(400, { error: validation })

        try {
            await updateFetchProvider(id, { config })
            return { success: true, message: 'Web fetch provider updated' }
        } catch (err) {
            console.error('Failed to update web fetch provider:', err)
            return fail(500, { error: 'Failed to update web fetch provider' })
        }
    },

    deleteFetchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        await deleteFetchProvider(id)
        return { success: true, message: 'Web fetch provider removed' }
    },

    setCurrentFetchProvider: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const id = formData.get('id') as string
        if (!id) return fail(400, { error: 'Provider ID is required' })
        await setCurrentFetchProvider(id)
        return { success: true, message: 'Current web fetch provider updated' }
    },

    savePolicy: async ({ request, locals }) => {
        requireAdmin(locals)
        const formData = await request.formData()
        const blocklist = parseBlocklist((formData.get('blocklist') as string) || '')
        const validation = validateBlocklist(blocklist)
        if (validation) return fail(400, { error: validation })
        await setGlobal(WEB_ACCESS_POLICY_KEY, { blocklist })
        return { success: true, message: 'Web access policy updated' }
    },
}
