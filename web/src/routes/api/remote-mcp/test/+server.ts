import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { AuthType } from '$lib/types'
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
    const bearerToken = typeof rawBearerToken === 'string' ? rawBearerToken : null

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
