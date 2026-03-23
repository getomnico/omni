import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import type { PageServerLoad } from './$types'

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

    const connectedSources = await sourcesRepository.getAll()
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

            availableIntegrations = Array.from(integrationMap.values())
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
