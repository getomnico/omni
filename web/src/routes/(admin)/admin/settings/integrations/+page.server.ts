import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { SourcesRepository } from '$lib/server/repositories/sources'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
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

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const repo = new SourcesRepository(locals.db)
    const connectedSources = await repo.getAll()
    const latestSyncRuns = await repo.getLatestSyncRuns()
    const googleConnectorConfig = await getConnectorConfigPublic('google')

    // Fetch registered connectors from connector manager
    const config = getConfig()
    let availableIntegrations: {
        id: string
        name: string
        description: string
        connected: boolean
    }[] = []

    try {
        const response = await fetch(`${config.services.connectorManagerUrl}/connectors`)
        if (response.ok) {
            const connectors: ConnectorInfo[] = await response.json()

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
        locals.logger.error('Failed to fetch connectors from connector manager', error)
    }

    return {
        connectedSources,
        latestSyncRuns,
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        availableIntegrations,
    }
}
