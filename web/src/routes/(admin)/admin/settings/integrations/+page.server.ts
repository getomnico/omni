import { requireAdmin } from '$lib/server/authHelpers'
import { sourcesRepository } from '$lib/server/repositories/sources'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const connectedSources = await sourcesRepository.getAll()
    const runningSyncs = await sourcesRepository.getRunningSyncs()

    return {
        connectedSources,
        runningSyncs,
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
            {
                id: 'slack',
                name: 'Slack',
                description: 'Connect to Slack messages and files using a bot token',
                connected: connectedSources.some((source) => source.sourceType === 'slack'),
                authType: 'bot_token',
            },
            {
                id: 'filesystem',
                name: 'Filesystem',
                description: 'Index local files and directories',
                connected: connectedSources.some((source) => source.sourceType === 'local_files'),
                authType: 'config_based',
            },
        ],
    }
}
