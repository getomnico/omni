import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

type SyncMode = 'incremental' | 'full'

async function getSyncMode(request: Request): Promise<SyncMode> {
    const body = await request.json().catch(() => {
        throw error(400, 'Expected JSON request body')
    })
    if (!body || typeof body !== 'object' || Array.isArray(body)) {
        throw error(400, 'Expected JSON object request body')
    }

    const mode = (body as { sync_mode?: unknown }).sync_mode ?? 'incremental'

    if (mode !== 'incremental' && mode !== 'full') {
        throw error(400, 'sync_mode must be "incremental" or "full"')
    }

    return mode
}

function getSyncModeLabel(mode: SyncMode) {
    return mode === 'full' ? 'Full sync' : 'Incremental sync'
}

export const POST: RequestHandler = async ({ params, request, fetch }) => {
    const { sourceId } = params

    if (!sourceId) {
        throw error(400, 'Source ID is required')
    }

    try {
        const mode = await getSyncMode(request)
        const config = getConfig()
        const connectorManagerUrl = config.services.connectorManagerUrl

        // Call connector-manager's mode-aware sync endpoint. Defaults to incremental.
        const syncResponse = await fetch(`${connectorManagerUrl}/sync`, {
            method: 'POST',
            headers: {
                'content-type': 'application/json',
            },
            body: JSON.stringify({
                source_id: sourceId,
                sync_mode: mode,
            }),
        })

        if (!syncResponse.ok) {
            let errorMessage = 'Failed to trigger sync'
            try {
                const errorBody = await syncResponse.json()
                errorMessage = errorBody.error || errorMessage
            } catch {
                // If response isn't JSON, use the text
                errorMessage = (await syncResponse.text()) || errorMessage
            }
            logger.error(`Sync failed for source ${sourceId}`, {
                error: errorMessage,
                status: syncResponse.status,
                syncMode: mode,
            })
            throw error(syncResponse.status, errorMessage)
        }

        const syncResult = await syncResponse.json()

        return json({
            success: true,
            message: `${getSyncModeLabel(mode)} triggered successfully`,
            sourceId,
            syncMode: mode,
            result: syncResult,
        })
    } catch (err) {
        logger.error('Error triggering sync:', err)

        if (err && typeof err === 'object' && 'status' in err) {
            throw err // Re-throw SvelteKit errors
        }

        throw error(500, 'Internal server error')
    }
}
