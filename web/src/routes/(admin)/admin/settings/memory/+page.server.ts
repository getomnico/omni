import { fail } from '@sveltejs/kit'
import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { getConfigValue, setConfigValue } from '$lib/server/db/configuration'
import { listAllActiveModels } from '$lib/server/db/model-providers'
import { getCurrentProvider } from '$lib/server/db/embedding-providers'
import type { PageServerLoad, Actions } from './$types'

const VALID_MODES = ['off', 'chat', 'full']

type StoredMemory = { id: string; memory: string; created_at?: string }

async function fetchMemories(userId: string): Promise<StoredMemory[]> {
    const { services } = getConfig()
    try {
        const resp = await fetch(`${services.aiServiceUrl}/memories`, {
            headers: { 'x-user-id': userId },
        })
        if (!resp.ok) return []
        const data = (await resp.json()) as { memories?: StoredMemory[] }
        return data.memories ?? []
    } catch (err) {
        console.error('Failed to fetch memories:', err)
        return []
    }
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const [orgDefaultConfig, memoryLlmConfig, models, embedder, memories] = await Promise.all([
        getConfigValue('memory_mode_default'),
        getConfigValue('memory_llm_id'),
        listAllActiveModels(),
        getCurrentProvider(),
        fetchMemories(locals.user!.id),
    ])

    const orgDefault = (orgDefaultConfig?.value as string) ?? 'off'
    const memoryLlmId = (memoryLlmConfig?.value as string) ?? ''
    const embedderAvailable = embedder !== null

    return { orgDefault, memoryLlmId, models, embedderAvailable, memories }
}

export const actions: Actions = {
    deleteOne: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const memoryId = (formData.get('memoryId') as string | null)?.trim()
        if (!memoryId) {
            return fail(400, { deleteError: 'Missing memory id' })
        }

        const { services } = getConfig()
        try {
            const resp = await fetch(
                `${services.aiServiceUrl}/memories/${encodeURIComponent(memoryId)}`,
                {
                    method: 'DELETE',
                    headers: { 'x-user-id': locals.user!.id },
                },
            )
            if (!resp.ok) {
                return fail(resp.status === 404 ? 404 : 502, {
                    deleteError: resp.status === 404 ? 'Memory not found' : 'Failed to delete memory',
                })
            }
            return { deleted: true }
        } catch (err) {
            console.error('Failed to delete memory:', err)
            return fail(502, { deleteError: 'Failed to delete memory' })
        }
    },

    deleteAll: async ({ locals }) => {
        requireAdmin(locals)

        const { services } = getConfig()
        try {
            const resp = await fetch(`${services.aiServiceUrl}/memories`, {
                method: 'DELETE',
                headers: { 'x-user-id': locals.user!.id },
            })
            if (!resp.ok) {
                return fail(502, { deleteError: 'Failed to delete memories' })
            }
            return { deletedAll: true }
        } catch (err) {
            console.error('Failed to delete all memories:', err)
            return fail(502, { deleteError: 'Failed to delete memories' })
        }
    },

    save: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const mode = formData.get('mode') as string
        const llmId = (formData.get('llmId') as string) ?? ''

        if (!VALID_MODES.includes(mode)) {
            return fail(400, { error: 'Invalid memory mode' })
        }

        const embedder = await getCurrentProvider()
        if (!embedder && mode !== 'off') {
            return fail(400, {
                error: 'Configure an embedding provider in Admin → Embeddings before enabling memory.',
            })
        }

        try {
            await Promise.all([
                setConfigValue('memory_mode_default', { value: mode }),
                llmId
                    ? setConfigValue('memory_llm_id', { value: llmId })
                    : setConfigValue('memory_llm_id', { value: '' }),
            ])
            return { success: true }
        } catch (err) {
            console.error('Failed to update memory settings:', err)
            return fail(500, { error: 'Failed to save settings' })
        }
    },
}
