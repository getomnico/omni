import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import {
    getAllConnectorConfigsPublic,
    getConnectorConfig,
    upsertConnectorConfig,
} from '$lib/server/db/connector-configs'
import { validateWindshiftServerUrl } from '$lib/server/windshift-server-config'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const configs = await getAllConnectorConfigsPublic()
    return json(configs)
}

export const POST: RequestHandler = async ({ locals, request }) => {
    if (!locals.user || locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const body = await request.json()
    const { provider, config } = body

    if (!provider || !config) {
        throw error(400, 'Missing provider or config')
    }

    const existing = await getConnectorConfig(provider)
    const existingConfig = (existing?.config ?? {}) as Record<string, unknown>
    const nextConfig = { ...existingConfig, ...config }

    if (!config.oauth_client_secret && existingConfig.oauth_client_secret) {
        nextConfig.oauth_client_secret = existingConfig.oauth_client_secret
    }

    // Windshift base URLs are admin-entered and fetched by the server (OAuth
    // endpoints, MCP) — enforce the same SSRF policy as remote MCP sources.
    if (provider === 'windshift') {
        try {
            await validateWindshiftServerUrl(
                'Windshift URL',
                typeof nextConfig.base_url === 'string' ? nextConfig.base_url : '',
            )
            await validateWindshiftServerUrl(
                'Windshift internal URL',
                typeof nextConfig.internal_base_url === 'string'
                    ? nextConfig.internal_base_url
                    : '',
            )
        } catch (err) {
            throw error(
                400,
                err instanceof Error ? err.message : 'Windshift server URLs are not allowed',
            )
        }
    }

    const result = await upsertConnectorConfig(provider, nextConfig, locals.user.id)
    return json({ provider: result.provider, updatedAt: result.updatedAt })
}
