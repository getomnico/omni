import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, getSourcesByType, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.TELEGRAM) {
        throw error(400, 'Invalid source type for this page')
    }

    // Collect chats already synced by OTHER Telegram sources (for dedup indicator)
    const allTelegramSources = await getSourcesByType(SourceType.TELEGRAM)
    const otherSyncedChats: Record<string, string> = {}
    for (const other of allTelegramSources) {
        if (other.id === source.id) continue
        const cfg = other.config as Record<string, any> | null
        const chats: string[] = cfg?.chats ?? []
        for (const chatName of chats) {
            otherSyncedChats[chatName] = other.name
        }
    }

    return {
        source,
        otherSyncedChats,
    }
}

export const actions: Actions = {
    default: async ({ request, params, locals }) => {
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }

        const source = await getSourceById(params.sourceId)
        if (!source) {
            throw error(404, 'Source not found')
        }

        if (source.sourceType !== SourceType.TELEGRAM) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()
        const isActive = formData.has('enabled')
        const selectedChats = formData.get('selected_chats') as string

        const config: Record<string, any> = { ...(source.config as Record<string, any>) }

        if (selectedChats) {
            try {
                const chatNames: string[] = JSON.parse(selectedChats)
                config.chats = chatNames
            } catch {
                // Keep existing config if JSON parse fails
            }
        }

        try {
            await updateSourceById(source.id, {
                isActive,
                config,
            })

            if (isActive) {
                const connectorManagerUrl = getConfig().services.connectorManagerUrl
                try {
                    await fetch(`${connectorManagerUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Telegram settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
