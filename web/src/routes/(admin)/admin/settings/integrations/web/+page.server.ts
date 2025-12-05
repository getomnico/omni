import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getWebSources, updateWebSource } from '$lib/server/db/sources'

export const load: PageServerLoad = async ({ params, url, locals }) => {
    requireAdmin(locals)

    const webSources = await getWebSources()

    if (webSources.length === 0) {
        throw error(404, 'No Web source found. Please connect Web crawler first.')
    }

    return {
        webSource: webSources[0],
    }
}

export const actions: Actions = {
    default: async ({ request, locals }) => {
        const session = locals.session
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
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
            await updateWebSource({
                isActive,
                rootUrl,
                maxDepth,
                maxPages,
                respectRobotsTxt,
                includeSubdomains,
                blacklistPatterns,
                userAgent,
            })

            if (isActive) {
                const webConnectorUrl = process.env.WEB_CONNECTOR_URL || 'http://localhost:3006'

                const webSources = await getWebSources()
                if (webSources.length > 0) {
                    const source = webSources[0]
                    try {
                        await fetch(`${webConnectorUrl}/sync/${source.id}`, {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json',
                            },
                        })
                    } catch (err) {
                        console.error(`Failed to trigger sync for web source ${source.id}:`, err)
                    }
                }
            }
        } catch (err) {
            console.error('Failed to save Web crawler settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations?success=web_configured')
    },
}
