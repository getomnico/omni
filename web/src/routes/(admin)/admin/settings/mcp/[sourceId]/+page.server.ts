import { error } from '@sveltejs/kit'
import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import {
    isClientConfigComplete,
    tokenEndpointAuthMethodForConfig,
} from '$lib/server/oauth/connectorOAuth'
import { IntegrationType } from '$lib/types'
import type { PageServerLoad } from './$types'

type RemoteMcpSourceResponse = {
    id: string
    name: string
    sourceType: string
    authType: string | null
    config: Record<string, unknown>
    isActive: boolean
}

type ConnectorInfo = {
    source_type: string
    manifest?: {
        integration_type?: string
        actions?: unknown[]
        resources?: unknown[]
        oauth?: Record<string, unknown> | null
    } | null
}

export const load: PageServerLoad = async ({ locals, fetch, params }) => {
    requireAdmin(locals)

    const sourceResponse = await fetch(`/api/remote-mcp/${params.sourceId}`)
    if (!sourceResponse.ok) {
        throw error(sourceResponse.status, 'Remote MCP source not found')
    }
    const source = (await sourceResponse.json()) as RemoteMcpSourceResponse

    let manifestAvailable = false
    let toolCount = 0
    let resourceCount = 0
    let oauthProvider = `remote_mcp:${source.sourceType}`
    let oauthManifest: Record<string, unknown> | null = null

    try {
        const response = await fetch(`${getConfig().services.connectorManagerUrl}/connectors`)
        if (response.ok) {
            const connectors = (await response.json()) as ConnectorInfo[]
            const entry = connectors.find(
                (connector) =>
                    connector.source_type === source.sourceType &&
                    connector.manifest?.integration_type === IntegrationType.REMOTE_MCP,
            )
            const manifest = entry?.manifest
            manifestAvailable = Boolean(manifest)
            toolCount = manifest?.actions?.length ?? 0
            resourceCount = manifest?.resources?.length ?? 0
            oauthManifest = manifest?.oauth ?? null
            if (typeof oauthManifest?.provider === 'string') oauthProvider = oauthManifest.provider
        }
    } catch (err) {
        locals.logger.warn('Failed to fetch remote MCP manifest status', err)
    }

    const oauthClient = await getConnectorConfigPublic(oauthProvider)
    const tokenEndpointAuthMethod = tokenEndpointAuthMethodForConfig(
        oauthClient?.config,
        oauthManifest as any,
    )

    return {
        source,
        manifest: { available: manifestAvailable, toolCount, resourceCount },
        oauth: {
            provider: oauthProvider,
            configured: isClientConfigComplete(oauthClient?.config, tokenEndpointAuthMethod),
            config: oauthClient?.config ?? {},
        },
    }
}
