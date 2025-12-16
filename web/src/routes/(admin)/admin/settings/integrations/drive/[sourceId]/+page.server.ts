import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById, type UserFilterMode } from '$lib/server/db/sources'
import { SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, 'Invalid source type for this page')
    }

    return {
        source,
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

        if (source.sourceType !== SourceType.GOOGLE_DRIVE) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const userFilterMode = (formData.get('userFilterMode') as UserFilterMode) || 'all'
        const userWhitelist =
            userFilterMode === 'whitelist' ? (formData.getAll('userWhitelist') as string[]) : null
        const userBlacklist =
            userFilterMode === 'blacklist' ? (formData.getAll('userBlacklist') as string[]) : null

        if (
            isActive &&
            userFilterMode === 'whitelist' &&
            (!userWhitelist || userWhitelist.length === 0)
        ) {
            throw error(400, 'Whitelist mode requires at least one user')
        }

        try {
            await updateSourceById(source.id, {
                isActive,
                userFilterMode,
                userWhitelist,
                userBlacklist,
            })

            if (isActive) {
                const googleConnectorUrl =
                    process.env.GOOGLE_CONNECTOR_URL || 'http://localhost:3003'
                try {
                    await fetch(`${googleConnectorUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Google Drive settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
