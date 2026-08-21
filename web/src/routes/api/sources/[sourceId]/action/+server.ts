import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { SourceType } from '$lib/types'

const FOLDER_DISCOVERY_ACTIONS = new Set(['discover_folders', 'discover_personal_folders'])
const FOLDER_DISCOVERY_AUTH_MODES = new Set(['domain_wide_delegation', 'service_account_direct'])

function validateFolderDiscoveryParams(action: string, actionParams: unknown): void {
    if (!FOLDER_DISCOVERY_ACTIONS.has(action)) return
    if (actionParams === undefined || actionParams === null) {
        throw error(400, `${action} params with a query are required`)
    }
    if (typeof actionParams !== 'object' || Array.isArray(actionParams)) {
        throw error(400, `${action} params must be a JSON object`)
    }

    const params = actionParams as Record<string, unknown>
    const allowedFields = new Set(['auth_mode', 'query'])
    for (const key of Object.keys(params)) {
        if (!allowedFields.has(key)) {
            throw error(400, `Unknown ${action} param: '${key}'`)
        }
    }
    if ('auth_mode' in params) {
        if (
            typeof params.auth_mode !== 'string' ||
            !FOLDER_DISCOVERY_AUTH_MODES.has(params.auth_mode)
        ) {
            throw error(400, `${action} auth_mode is invalid`)
        }
    }
    if (!('query' in params)) {
        throw error(400, `${action} query is required`)
    }
    if (typeof params.query !== 'string') {
        throw error(400, `${action} query must be a string`)
    }
    const queryLength = Array.from(params.query.trim()).length
    if (queryLength < 2 || queryLength > 200) {
        throw error(400, `${action} query must contain 2–200 characters`)
    }
}

// This route is only meant to handle "actions" that originate in the UI.
//
// For most AI-driven actions, omni-ai will directly call the connector mgr, i.e.,
// all connector tool calls, mcp tool calls etc. do not go through here.
//
// This route is specifically meant for some niche interactions, e.g., search for users
// in google workspace in the omni UI.
// This is an action in the google connector and so we need a way to invoke this action from here.
export const POST: RequestHandler = async ({ params, locals, request }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
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

    // Personal users may only browse folders on their own Google Drive source.
    // All other connector actions remain admin-only. Keep this fast path before
    // source lookup so admin-only actions retain their existing behavior.
    if (locals.user.role !== 'admin' && action !== 'discover_personal_folders') {
        throw error(403, 'Admin access required')
    }

    if (locals.user.role !== 'admin') {
        const source = await sourcesRepository.getById(sourceId)
        if (!source || source.isDeleted) {
            throw error(404, 'Source not found')
        }
        if (
            source.sourceType !== SourceType.GOOGLE_DRIVE ||
            source.scope !== 'user' ||
            source.createdBy !== locals.user.id
        ) {
            throw error(403, 'Admin access required')
        }
    }

    validateFolderDiscoveryParams(action, actionParams)

    try {
        const config = getConfig()
        const connectorManagerUrl = config.services.connectorManagerUrl

        const response = await fetch(`${connectorManagerUrl}/action`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                source_id: sourceId,
                user_id: locals.user.id,
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
