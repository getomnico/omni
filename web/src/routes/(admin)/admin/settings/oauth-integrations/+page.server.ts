import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { getAllConnectorConfigsPublic } from '$lib/server/db/connector-configs'
import { callbackUrl, type OAuthManifestConfig } from '$lib/server/oauth/connectorOAuth'
import { getSourceDisplayName } from '$lib/utils/icons'
import type { SourceType } from '$lib/types'
import type { PageServerLoad } from './$types'

interface ConnectorInfo {
    source_type: string
    manifest?: {
        connector_id?: string
        display_name?: string
        source_types?: string[]
        oauth?: OAuthManifestConfig | null
    } | null
}

export interface OAuthIntegrationProvider {
    provider: string
    displayName: string
    sourceTypes: string[]
    sourceTypeNames: string[]
    configured: boolean
    updatedAt: Date | null
    config: Record<string, unknown>
}

const PROVIDER_DISPLAY_NAMES: Record<string, string> = {
    github: 'GitHub',
    google: 'Google',
    microsoft: 'Microsoft',
}

function providerDisplayName(provider: string, connectors: ConnectorInfo[]): string {
    if (PROVIDER_DISPLAY_NAMES[provider]) return PROVIDER_DISPLAY_NAMES[provider]
    const connector = connectors.find((c) => c.manifest?.oauth?.provider === provider)
    if (connector?.manifest?.display_name) return connector.manifest.display_name
    return provider
        .split(/[_-]/)
        .map((part) => (part ? `${part[0].toUpperCase()}${part.slice(1)}` : part))
        .join(' ')
}

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const config = getConfig()
    const savedConfigs = await getAllConnectorConfigsPublic()
    const savedByProvider = new Map(savedConfigs.map((row) => [row.provider, row]))

    let providers: OAuthIntegrationProvider[] = []

    try {
        const response = await fetch(`${config.services.connectorManagerUrl}/connectors`)
        if (response.ok) {
            const connectors = (await response.json()) as ConnectorInfo[]
            const sourceTypesByProvider = new Map<string, Set<string>>()

            for (const connector of connectors) {
                const oauth = connector.manifest?.oauth
                if (!oauth?.provider) continue

                const sourceTypes = connector.manifest?.source_types?.length
                    ? connector.manifest.source_types
                    : [connector.source_type]

                if (!sourceTypesByProvider.has(oauth.provider)) {
                    sourceTypesByProvider.set(oauth.provider, new Set())
                }
                const set = sourceTypesByProvider.get(oauth.provider)!
                for (const sourceType of sourceTypes) {
                    if (oauth.scopes[sourceType]) set.add(sourceType)
                }
                if (set.size === 0) {
                    for (const sourceType of sourceTypes) set.add(sourceType)
                }
            }

            providers = Array.from(sourceTypesByProvider.entries())
                .map(([provider, sourceTypesSet]) => {
                    const saved = savedByProvider.get(provider)
                    const sourceTypes = Array.from(sourceTypesSet).sort()
                    return {
                        provider,
                        displayName: providerDisplayName(provider, connectors),
                        sourceTypes,
                        sourceTypeNames: sourceTypes.map(
                            (sourceType) =>
                                getSourceDisplayName(sourceType as SourceType) ?? sourceType,
                        ),
                        configured: !!(
                            saved?.config?.oauth_client_id && saved?.config?.oauth_client_secret
                        ),
                        updatedAt: saved?.updatedAt ?? null,
                        config: saved?.config ?? {},
                    }
                })
                .sort((a, b) => a.displayName.localeCompare(b.displayName))
        }
    } catch (error) {
        locals.logger.error('Failed to fetch OAuth-capable connectors', error)
    }

    return {
        providers,
        redirectUri: callbackUrl(),
    }
}
