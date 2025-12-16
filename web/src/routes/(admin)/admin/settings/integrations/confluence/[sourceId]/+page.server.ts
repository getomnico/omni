import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { SourceType, type ConfluenceSourceConfig } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.CONFLUENCE) {
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

        if (source.sourceType !== SourceType.CONFLUENCE) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const siteUrl = formData.get('siteUrl') as string | null
        const spaceFilters = formData.getAll('spaceFilters') as string[]

        try {
            const existingConfig = (source.config as ConfluenceSourceConfig) || {}
            const config: ConfluenceSourceConfig = {
                base_url: siteUrl
                    ? siteUrl.startsWith('http')
                        ? siteUrl
                        : `https://${siteUrl}`
                    : existingConfig.base_url,
                space_filters: spaceFilters.length > 0 ? spaceFilters : undefined,
            }

            await updateSourceById(source.id, {
                isActive,
                config,
            })

            if (isActive) {
                const atlassianConnectorUrl =
                    process.env.ATLASSIAN_CONNECTOR_URL || 'http://localhost:3005'
                try {
                    await fetch(`${atlassianConnectorUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Confluence settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
