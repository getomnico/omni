import { redirect, fail } from '@sveltejs/kit'
import { getConfig } from '$lib/server/config'
import { deleteUser, getGlobal, setUser } from '$lib/server/db/configuration'
import { getCurrentProvider } from '$lib/server/db/embedding-providers'
import type { PageServerLoad, Actions } from './$types'

const MODE_RANK: Record<string, number> = { off: 0, chat: 1, full: 2 }
const VALID_MODES = ['off', 'chat', 'full', '']

type StoredMemory = { id: string; memory: string; created_at?: string }

function isAllowed(candidate: string, ceiling: string): boolean {
    if (candidate === '') return true
    const c = MODE_RANK[candidate]
    const max = MODE_RANK[ceiling] ?? 0
    return c !== undefined && c <= max
}

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
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const [orgDefaultConfig, embedder, memories] = await Promise.all([
        getGlobal('memory_mode_default'),
        getCurrentProvider(),
        fetchMemories(locals.user.id),
    ])
    const orgDefault = (orgDefaultConfig?.value as string) ?? 'off'

    return {
        currentMode: locals.user.memoryMode ?? null,
        orgDefault,
        embedderAvailable: embedder !== null,
        memories,
    }
}

export const actions: Actions = {
    save: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const mode = formData.get('mode') as string

        if (!VALID_MODES.includes(mode)) {
            return fail(400, { error: 'Invalid memory mode' })
        }

        const [orgDefaultConfig, embedder] = await Promise.all([
            getGlobal('memory_mode_default'),
            getCurrentProvider(),
        ])
        const orgDefault = (orgDefaultConfig?.value as string) ?? 'off'

        if (!embedder && mode !== 'off' && mode !== '') {
            return fail(400, {
                error: 'Memory is unavailable until an embedding provider is configured.',
            })
        }

        if (!isAllowed(mode, orgDefault)) {
            return fail(400, {
                error: `Your admin allows up to "${orgDefault}". Pick a lower or equal option.`,
            })
        }

        try {
            if (mode === '') {
                await deleteUser(locals.user.id, 'memory_mode')
            } else {
                await setUser(locals.user.id, 'memory_mode', { value: mode })
            }
            return { success: true }
        } catch (err) {
            console.error('Failed to update memory mode:', err)
            return fail(500, { error: 'Failed to save preference' })
        }
    },

    deleteOne: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

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
                    headers: { 'x-user-id': locals.user.id },
                },
            )
            if (!resp.ok) {
                return fail(resp.status === 404 ? 404 : 502, {
                    deleteError:
                        resp.status === 404 ? 'Memory not found' : 'Failed to delete memory',
                })
            }
            return { deleted: true }
        } catch (err) {
            console.error('Failed to delete memory:', err)
            return fail(502, { deleteError: 'Failed to delete memory' })
        }
    },

    deleteAll: async ({ locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const { services } = getConfig()
        try {
            const resp = await fetch(`${services.aiServiceUrl}/memories`, {
                method: 'DELETE',
                headers: { 'x-user-id': locals.user.id },
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
}
