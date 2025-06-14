import { requireAuth } from '$lib/server/authHelpers'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    await requireAuth(locals.user)

    // Get all organization-level connected sources (read-only for users)
    const connectedSources = await db.select().from(sources)

    return {
        connectedSources,
        availableIntegrations: [
            {
                id: 'google',
                name: 'Google Workspace',
                description: 'Google Drive, Docs, Gmail, and more',
                icon: '🔗',
                connected: connectedSources.some((source) => source.sourceType === 'google'),
            },
            {
                id: 'slack',
                name: 'Slack',
                description: 'Slack messages and files',
                icon: '💬',
                connected: connectedSources.some((source) => source.sourceType === 'slack'),
            },
            {
                id: 'confluence',
                name: 'Confluence',
                description: 'Atlassian Confluence pages',
                icon: '📚',
                connected: connectedSources.some((source) => source.sourceType === 'confluence'),
            },
        ],
    }
}
