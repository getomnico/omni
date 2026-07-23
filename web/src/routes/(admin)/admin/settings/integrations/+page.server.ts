import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { getAllConnectorConfigsPublic } from '$lib/server/db/connector-configs'
import {
    callbackUrl,
    isAutoManagedOAuthProvider,
    isClientConfigComplete,
    tokenEndpointAuthMethodForConfig,
    type OAuthManifestConfig,
} from '$lib/server/oauth/connectorOAuth'
import type { SyncRun } from '$lib/server/db/schema'
import type { PageServerLoad } from './$types'

const CONNECTOR_DISPLAY_ORDER: string[] = [
    // Productivity suites
    'google',
    'google_ads',
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
    // HRIS
    'darwinbox',
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
        oauth?: OAuthManifestConfig | null
    }
}

export interface OAuthIntegrationProvider {
    provider: string
    displayName: string
    configured: boolean
    updatedAt: Date | null
    config: Record<string, unknown>
}

function providerDisplayName(provider: string, connectors: ConnectorInfo[]): string {
    const connector = connectors.find((c) => c.manifest?.oauth?.provider === provider)
    if (connector?.manifest?.display_name) return connector.manifest.display_name
    return provider
        .split(/[_-]/)
        .map((part) => (part ? `${part[0].toUpperCase()}${part.slice(1)}` : part))
        .join(' ')
}

interface ConnectorManagerSourceOverview {
    source: {
        id: string
    }
    health: 'healthy' | 'unhealthy'
    sync_runs: Record<string, string | number | null>[]
}

function mapSyncRun(run: Record<string, string | number | null>): SyncRun {
    return {
        id: run.id as string,
        sourceId: run.source_id as string,
        syncType: run.sync_type as string,
        startedAt: new Date(run.started_at as string),
        completedAt: run.completed_at ? new Date(run.completed_at as string) : null,
        status: run.status as string,
        documentsScanned: run.documents_scanned as number | null,
        documentsProcessed: run.documents_processed as number | null,
        documentsUpdated: run.documents_updated as number | null,
        errorMessage: run.error_message as string | null,
        createdAt: new Date(run.created_at as string),
        updatedAt: new Date(run.updated_at as string),
    }
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const connectedSources = await sourcesRepository.getOrgWide()
    const latestSyncRuns = await sourcesRepository.getLatestSyncRuns()
    const savedOAuthConfigs = await getAllConnectorConfigsPublic()
    const savedOAuthConfigByProvider = new Map(savedOAuthConfigs.map((row) => [row.provider, row]))

    // Fetch registered connectors from connector manager
    const config = getConfig()
    let availableIntegrations: {
        id: string
        name: string
        description: string
        connected: boolean
    }[] = []
    let oauthProviders: OAuthIntegrationProvider[] = []
    const sourceHealth = new Map<string, 'healthy' | 'unhealthy'>()

    try {
        const [connectorsResponse, sourcesResponse] = await Promise.all([
            fetch(`${config.services.connectorManagerUrl}/connectors`),
            fetch(`${config.services.connectorManagerUrl}/sources`),
        ])

        if (sourcesResponse.ok) {
            const overviews = (await sourcesResponse.json()) as ConnectorManagerSourceOverview[]
            for (const overview of overviews) {
                sourceHealth.set(overview.source.id, overview.health)
                if (overview.sync_runs[0]) {
                    latestSyncRuns.set(overview.source.id, mapSyncRun(overview.sync_runs[0]))
                }
            }
        }

        if (connectorsResponse.ok) {
            const connectors: ConnectorInfo[] = await connectorsResponse.json()

            // Group by connector_id to build integration list
            const integrationMap = new Map<
                string,
                {
                    id: string
                    name: string
                    description: string
                    connected: boolean
                }
            >()
            const sourceTypesByOAuthProvider = new Map<string, Set<string>>()
            const oauthManifestByProvider = new Map<string, OAuthManifestConfig>()

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

                const oauth = connector.manifest?.oauth
                if (oauth?.provider) {
                    oauthManifestByProvider.set(oauth.provider, oauth)
                    const sourceTypes = connector.manifest?.source_types?.length
                        ? connector.manifest.source_types
                        : [connector.source_type]

                    if (!sourceTypesByOAuthProvider.has(oauth.provider)) {
                        sourceTypesByOAuthProvider.set(oauth.provider, new Set())
                    }
                    const set = sourceTypesByOAuthProvider.get(oauth.provider)!
                    for (const sourceType of sourceTypes) {
                        if (oauth.scopes[sourceType]) set.add(sourceType)
                    }
                    if (set.size === 0) {
                        for (const sourceType of sourceTypes) set.add(sourceType)
                    }
                }
            }

            oauthProviders = Array.from(sourceTypesByOAuthProvider.keys())
                .filter(
                    (provider) =>
                        !isAutoManagedOAuthProvider(oauthManifestByProvider.get(provider)),
                )
                .map((provider) => {
                    const saved = savedOAuthConfigByProvider.get(provider)
                    const manifest = oauthManifestByProvider.get(provider)
                    const tokenEndpointAuthMethod = tokenEndpointAuthMethodForConfig(
                        saved?.config,
                        manifest,
                    )
                    return {
                        provider,
                        displayName: providerDisplayName(provider, connectors),
                        configured: isClientConfigComplete(saved?.config, tokenEndpointAuthMethod),
                        updatedAt: saved?.updatedAt ?? null,
                        config: saved?.config ?? {},
                    }
                })
                .sort((a, b) => a.displayName.localeCompare(b.displayName))

            availableIntegrations = Array.from(integrationMap.values())
                // Windshift is a personal OAuth source. Users connect it under My Integrations.
                .filter((integration) => integration.id !== 'windshift')
                .sort((a, b) => {
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
        availableIntegrations,
        oauthProviders,
        oauthRedirectUri: callbackUrl(),
    }
}
