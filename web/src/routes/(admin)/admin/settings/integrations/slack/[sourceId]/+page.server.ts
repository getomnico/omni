import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.SLACK) {
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

        if (source.sourceType !== SourceType.SLACK) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()
        const isActive = formData.has('enabled')

        try {
            await updateSourceById(source.id, {
                isActive,
                config: source.config || {},
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
            console.error('Failed to save Slack settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
