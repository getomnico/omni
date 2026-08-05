import { requireAdmin } from '$lib/server/authHelpers'
import { getConfig } from '$lib/server/config'
import { IntegrationType } from '$lib/types'
import type { PageServerLoad } from './$types'

type RemoteMcpSourceResponse = {
    id: string
    name: string
    sourceType: string
    authType: string | null
    config: Record<string, unknown>
    isActive: boolean
    hasCredential: boolean
}

type ConnectorInfo = {
    source_type: string
    manifest?: {
        integration_type?: string
        actions?: unknown[]
        resources?: unknown[]
    } | null
}

export const load: PageServerLoad = async ({ locals, fetch }) => {
    requireAdmin(locals)

    const sourceResponse = await fetch('/api/remote-mcp')
    const sources: RemoteMcpSourceResponse[] = sourceResponse.ok ? await sourceResponse.json() : []

    let manifestBySourceType = new Map<string, { toolCount: number; resourceCount: number }>()
    try {
        const config = getConfig()
        const response = await fetch(`${config.services.connectorManagerUrl}/connectors`)
        if (response.ok) {
            const connectors: ConnectorInfo[] = await response.json()
            for (const c of connectors) {
                if (c.manifest?.integration_type === IntegrationType.REMOTE_MCP) {
                    manifestBySourceType.set(c.source_type, {
                        toolCount: c.manifest.actions?.length ?? 0,
                        resourceCount: c.manifest.resources?.length ?? 0,
                    })
                }
            }
        }
    } catch (err) {
        locals.logger.warn('Failed to fetch MCP manifest status', err)
    }

    return { sources, manifestBySourceType: Object.fromEntries(manifestBySourceType) }
}
