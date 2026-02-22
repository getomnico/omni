import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import {
    getAllConnectorConfigsPublic,
    upsertConnectorConfig,
} from '$lib/server/db/connector-configs'

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

    const result = await upsertConnectorConfig(provider, config, locals.user.id)
    return json({ provider: result.provider, updatedAt: result.updatedAt })
}
