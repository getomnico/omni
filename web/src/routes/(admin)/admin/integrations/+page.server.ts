import { requireAdmin } from '$lib/server/authHelpers'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { syncRunsRepository } from '$lib/server/repositories/sync-runs'
import { documentsRepository } from '$lib/server/repositories/documents'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const connectedSources = await sourcesRepository.getAll()
    const latestSyncRuns = await syncRunsRepository.getLatest(10)
    const documentsBySource = await documentsRepository.getCountsBySource()

    const totalDocumentsIndexed = documentsBySource
        .map((r) => r.count)
        .reduce((a, v) => (a += v), 0)

    const documentCountBySource = documentsBySource.reduce(
        (acc, item) => {
            acc[item.sourceId] = item.count
            return acc
        },
        {} as Record<string, number>,
    )

    return {
        connectedSources,
        latestSyncRuns,
        documentStats: {
            totalDocumentsIndexed,
            documentsBySource: documentCountBySource,
        },
        availableIntegrations: [
            {
                id: 'google',
                name: 'Google Workspace',
                description:
                    'Connect to Google Drive, Docs, Gmail, and more using a service account',
                connected: connectedSources.some(
                    (source) =>
                        source.sourceType === 'google_drive' || source.sourceType === 'gmail',
                ),
                authType: 'service_account',
            },
            {
                id: 'slack',
                name: 'Slack',
                description: 'Connect to Slack messages and files using a bot token',
                connected: connectedSources.some((source) => source.sourceType === 'slack'),
                authType: 'bot_token',
            },
            {
                id: 'atlassian',
                name: 'Atlassian',
                description: 'Connect to Confluence and Jira using an API token',
                connected: connectedSources.some(
                    (source) => source.sourceType === 'confluence' || source.sourceType === 'jira',
                ),
                authType: 'api_token',
            },
            {
                id: 'web',
                name: 'Web',
                description: 'Index content from websites and documentation sites',
                connected: connectedSources.some((source) => source.sourceType === 'web'),
                authType: 'config_based',
            },
        ],
    }
}
