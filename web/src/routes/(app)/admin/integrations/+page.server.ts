import { requireAdmin } from '$lib/server/authHelpers'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    await requireAdmin(locals.user)

    // Get all organization-level connected sources
    const connectedSources = await db.select().from(sources)

    return {
        connectedSources,
        availableIntegrations: [
            {
                id: 'google',
                name: 'Google Workspace',
                description: 'Connect to Google Drive, Docs, Gmail, and more',
                icon: '🔗',
                connected: connectedSources.some((source) => source.sourceType === 'google'),
                connectUrl: '/api/oauth/google/connect',
            },
            {
                id: 'slack',
                name: 'Slack',
                description: 'Connect to Slack messages and files',
                icon: '💬',
                connected: connectedSources.some((source) => source.sourceType === 'slack'),
                connectUrl: '/api/oauth/slack/connect',
            },
            {
                id: 'confluence',
                name: 'Confluence',
                description: 'Connect to Atlassian Confluence pages',
                icon: '📚',
                connected: connectedSources.some((source) => source.sourceType === 'confluence'),
                connectUrl: '/api/oauth/confluence/connect',
            },
        ],
    }
}
