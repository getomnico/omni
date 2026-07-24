import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { and, eq, isNull } from 'drizzle-orm'
import { db } from '$lib/server/db'
import { serviceCredentials } from '$lib/server/db/schema'
import { decryptConfig } from '$lib/server/crypto/encryption'
import { AuthType, ServiceProvider } from '$lib/types'
import { probeRemoteMcpServer, remoteMcpConfigFromInput } from '$lib/server/mcp/client'

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (locals.user.role !== 'admin') throw error(403, 'Admin access required')

    const body = await request.json()
    const config = remoteMcpConfigFromInput({
        endpointUrl: String(body.endpointUrl ?? body.endpoint_url ?? ''),
        authType: body.authType ?? body.auth_type ?? null,
        writeToolsEnabled: body.writeToolsEnabled ?? body.write_tools_enabled,
    })
    const rawBearerToken = body.bearerToken ?? body.bearer_token
    let bearerToken = typeof rawBearerToken === 'string' ? rawBearerToken : null

    // If no token was provided but a sourceId is given, look up the stored credential
    if (config.auth_type === AuthType.BEARER_TOKEN && !bearerToken) {
        const sourceId = body.sourceId ?? body.source_id
        if (sourceId) {
            const credential = await db.query.serviceCredentials.findFirst({
                where: and(
                    eq(serviceCredentials.sourceId, sourceId),
                    eq(serviceCredentials.provider, ServiceProvider.REMOTE_MCP),
                    isNull(serviceCredentials.userId),
                ),
            })
            if (credential) {
                const decrypted = decryptConfig(credential.credentials)
                bearerToken = typeof decrypted.token === 'string' ? decrypted.token : null
            }
        }
    }

    if (config.auth_type === AuthType.BEARER_TOKEN && !bearerToken) {
        throw error(400, 'bearerToken is required for bearer auth')
    }

    const probe = await probeRemoteMcpServer({
        endpointUrl: config.endpoint_url,
        authType: config.auth_type,
        bearerToken,
    })

    return json(probe, { status: probe.ok ? 200 : 400 })
}
