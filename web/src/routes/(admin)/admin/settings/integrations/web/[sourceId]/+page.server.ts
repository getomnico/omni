import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType, type WebSourceConfig } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.WEB) {
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

        if (source.sourceType !== SourceType.WEB) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('webEnabled')
        const rootUrl = (formData.get('rootUrl') as string) || ''
        const maxDepth = parseInt(formData.get('maxDepth') as string) || 10
        const maxPages = parseInt(formData.get('maxPages') as string) || 10000
        const respectRobotsTxt = formData.has('respectRobotsTxt')
        const includeSubdomains = formData.has('includeSubdomains')
        const blacklistPatterns = formData.getAll('blacklistPatterns') as string[]
        const userAgent = (formData.get('userAgent') as string) || null

        if (isActive && !rootUrl.trim()) {
            throw error(400, 'Root URL is required when web crawler is enabled')
        }

        try {
            const config: WebSourceConfig = {
                root_url: rootUrl,
                max_depth: maxDepth,
                max_pages: maxPages,
                respect_robots_txt: respectRobotsTxt,
                include_subdomains: includeSubdomains,
                blacklist_patterns: blacklistPatterns,
                user_agent: userAgent,
            }

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
                    console.error(`Failed to trigger sync for web source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Web crawler settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations?success=web_configured')
    },
}
