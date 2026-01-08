import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

export const POST: RequestHandler = async ({ params, fetch }) => {
    const { sourceId } = params

    if (!sourceId) {
        throw error(400, 'Source ID is required')
    }

    try {
        const config = getConfig()
        const connectorManagerUrl = config.services.connectorManagerUrl

        // Call connector-manager's sync endpoint
        const syncResponse = await fetch(`${connectorManagerUrl}/sync/${sourceId}`, {
            method: 'POST',
        })

        if (!syncResponse.ok) {
            const errorText = await syncResponse.text()
            logger.error(`Sync failed for source ${sourceId}`, {
                error: errorText,
                status: syncResponse.status,
            })
            throw error(500, 'Failed to trigger sync')
        }

        const syncResult = await syncResponse.json()

        return json({
            success: true,
            message: 'Sync triggered successfully',
            sourceId,
            result: syncResult,
        })
    } catch (err) {
        console.error('Error triggering sync:', err)

        if (err && typeof err === 'object' && 'status' in err) {
            throw err // Re-throw SvelteKit errors
        }

        throw error(500, 'Internal server error')
    }
}
