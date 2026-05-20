import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import type { ConnectorManagerSyncRun, SourceSyncOverview } from '$lib/types'
import type { PageServerLoad } from './$types'

const CONNECTOR_DISPLAY_ORDER: string[] = [
    // Productivity suites
    'google',
    'microsoft',
    'atlassian',
    // Communication
    'slack',
    'gmail',
    'imap',
    // Knowledge & docs
    'notion',
    'confluence',
    // Project management
    'linear',
    'jira',
    'clickup',
    // Dev tools
    'github',
    // CRM & sales
    'hubspot',
    // Meetings
    'fireflies',
    // Other
    'nextcloud',
    'web',
    'filesystem',
    'paperless_ngx',
]

interface ConnectorInfo {
    source_type: string
    url: string
    healthy: boolean
    manifest?: {
        connector_id?: string
        display_name?: string
        description?: string
        source_types?: string[]
    }
}

function mapSyncRun(run: ConnectorManagerSyncRun) {
    return {
        id: run.id,
        sourceId: run.source_id,
        syncType: run.sync_type,
        startedAt: run.started_at ? new Date(run.started_at) : null,
        completedAt: run.completed_at ? new Date(run.completed_at) : null,
        status: run.status,
        documentsScanned: run.documents_scanned,
        documentsProcessed: run.documents_processed,
        documentsUpdated: run.documents_updated,
        errorMessage: run.error_message,
        createdAt: new Date(run.created_at),
        updatedAt: new Date(run.updated_at),
    }
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const connectedSources = await sourcesRepository.getOrgWide()
    const latestSyncRuns = await sourcesRepository.getLatestSyncRuns()
    const googleConnectorConfig = await getConnectorConfigPublic('google')

    // Fetch registered connectors from connector manager
    const config = getConfig()
    let availableIntegrations: {
        id: string
        name: string
        description: string
        connected: boolean
    }[] = []
    const sourceHealth = new Map<string, 'healthy' | 'unhealthy'>()

    try {
        const [connectorsResponse, sourcesResponse] = await Promise.all([
            fetch(`${config.services.connectorManagerUrl}/connectors`),
            fetch(`${config.services.connectorManagerUrl}/sources`),
        ])

        if (sourcesResponse.ok) {
            const overviews: SourceSyncOverview[] = await sourcesResponse.json()
            for (const overview of overviews) {
                sourceHealth.set(overview.source.id, overview.health)
                if (overview.sync_runs[0]) {
                    latestSyncRuns.set(overview.source.id, mapSyncRun(overview.sync_runs[0]) as any)
                }
            }
        }

        if (connectorsResponse.ok) {
            const connectors: ConnectorInfo[] = await connectorsResponse.json()

            // Group by connector_id to build integration list
            const integrationMap = new Map<
                string,
                { id: string; name: string; description: string; connected: boolean }
            >()

            for (const connector of connectors) {
                const connectorId = connector.manifest?.connector_id ?? connector.source_type
                if (!integrationMap.has(connectorId)) {
                    integrationMap.set(connectorId, {
                        id: connectorId,
                        name: connector.manifest?.display_name ?? connectorId,
                        description: connector.manifest?.description ?? '',
                        connected: false,
                    })
                }
                const integration = integrationMap.get(connectorId)!
                if (connectedSources.some((s) => s.sourceType === connector.source_type)) {
                    integration.connected = true
                }
            }

            availableIntegrations = Array.from(integrationMap.values()).sort((a, b) => {
                const idxA = CONNECTOR_DISPLAY_ORDER.indexOf(a.id)
                const idxB = CONNECTOR_DISPLAY_ORDER.indexOf(b.id)
                const orderA = idxA === -1 ? CONNECTOR_DISPLAY_ORDER.length : idxA
                const orderB = idxB === -1 ? CONNECTOR_DISPLAY_ORDER.length : idxB
                return orderA !== orderB ? orderA - orderB : a.id.localeCompare(b.id)
            })
        }
    } catch (error) {
        locals.logger.error('Failed to fetch connector manager data', error)
    }

    return {
        connectedSources,
        latestSyncRuns,
        sourceHealth,
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        availableIntegrations,
    }
}
