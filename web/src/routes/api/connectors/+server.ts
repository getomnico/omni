import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

export const GET: RequestHandler = async ({ locals, fetch: serverFetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const config = getConfig()
    const cmUrl = config.services.connectorManagerUrl
    if (!cmUrl) {
        throw error(502, 'Connector manager URL not configured')
    }

    try {
        const response = await serverFetch(`${cmUrl}/connectors`)
        if (!response.ok) {
            throw error(response.status as 502, 'Failed to fetch connectors from manager')
        }
        const data = await response.json()
        return json(data)
    } catch (e) {
        logger.error('Failed to proxy connectors request', e)
        throw error(502, 'Connector manager unavailable')
    }
}
