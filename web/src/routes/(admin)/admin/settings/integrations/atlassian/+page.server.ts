import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getAtlassianSources,
    updateAtlassianSources,
    getActiveAtlassianSources,
} from '$lib/server/db/sources'
import type { ConfluenceSourceConfig, JiraSourceConfig } from '$lib/types'

export const load: PageServerLoad = async ({ params, url, locals }) => {
    requireAdmin(locals)

    const atlassianSources = await getAtlassianSources()

    if (atlassianSources.length === 0) {
        throw error(404, 'No Atlassian sources found. Please connect Atlassian first.')
    }

    const jiraSource = atlassianSources.find((s) => s.sourceType === 'jira')
    const confluenceSource = atlassianSources.find((s) => s.sourceType === 'confluence')

    return {
        sources: atlassianSources,
        jiraSource: jiraSource || null,
        confluenceSource: confluenceSource || null,
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

        const jiraEnabled = formData.has('jiraEnabled')
        const confluenceEnabled = formData.has('confluenceEnabled')

        const jiraApiToken = formData.get('jiraApiToken') as string | null
        const jiraSiteUrl = formData.get('jiraSiteUrl') as string | null
        const jiraProjectFilters = formData.getAll('jiraProjectFilters') as string[]

        const confluenceApiToken = formData.get('confluenceApiToken') as string | null
        const confluenceSiteUrl = formData.get('confluenceSiteUrl') as string | null
        const confluenceSpaceFilters = formData.getAll('confluenceSpaceFilters') as string[]

        try {
            let jiraConfig: JiraSourceConfig | undefined = undefined
            if (jiraSiteUrl) {
                jiraConfig = {
                    base_url: jiraSiteUrl.startsWith('http')
                        ? jiraSiteUrl
                        : `https://${jiraSiteUrl}`,
                    project_filters: jiraProjectFilters.length > 0 ? jiraProjectFilters : undefined,
                }
            }

            let confluenceConfig: ConfluenceSourceConfig | undefined = undefined
            if (confluenceSiteUrl) {
                confluenceConfig = {
                    base_url: confluenceSiteUrl.startsWith('http')
                        ? confluenceSiteUrl
                        : `https://${confluenceSiteUrl}`,
                    space_filters:
                        confluenceSpaceFilters.length > 0 ? confluenceSpaceFilters : undefined,
                }
            }

            await updateAtlassianSources(
                jiraEnabled,
                confluenceEnabled,
                jiraConfig,
                confluenceConfig,
            )

            const atlassianConnectorUrl =
                process.env.ATLASSIAN_CONNECTOR_URL || 'http://localhost:3005'

            const activeSources = await getActiveAtlassianSources()

            for (const source of activeSources) {
                try {
                    await fetch(`${atlassianConnectorUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                        },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Atlassian integration settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations?success=atlassian_configured')
    },
}
