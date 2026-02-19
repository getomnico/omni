import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

export const POST: RequestHandler = async ({ params, locals, request }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const { sourceId } = params
    if (!sourceId) {
        throw error(400, 'Source ID is required')
    }

    const body = await request.json()
    const { action, params: actionParams } = body

    if (!action) {
        throw error(400, 'Action is required')
    }

    try {
        const config = getConfig()
        const connectorManagerUrl = config.services.connectorManagerUrl

        const response = await fetch(`${connectorManagerUrl}/action`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                source_id: sourceId,
                action,
                params: actionParams || {},
            }),
        })

        if (!response.ok) {
            let errorMessage = 'Failed to execute action'
            try {
                const errorBody = await response.json()
                errorMessage = errorBody.error || errorMessage
            } catch {
                errorMessage = (await response.text()) || errorMessage
            }
            logger.error(`Action ${action} failed for source ${sourceId}`, {
                error: errorMessage,
                status: response.status,
            })
            throw error(response.status, errorMessage)
        }

        const result = await response.json()
        return json(result)
    } catch (err) {
        if (err && typeof err === 'object' && 'status' in err) {
            throw err
        }
        logger.error('Error executing action:', err)
        throw error(500, 'Internal server error')
    }
}
