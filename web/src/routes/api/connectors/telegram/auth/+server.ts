import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

// Pre-source auth action dispatch for the Telegram connector.
//
// Routes through connector-manager's `/connectors/:source_type/action`
// endpoint, which checks the action's `authenticated` flag in the manifest
// and skips credential loading for unauthenticated actions like
// `auth_send_code` and `auth_verify_code`.
//
// Body: { action: "auth_send_code" | "auth_verify_code", params: {...} }

const ALLOWED_ACTIONS = new Set(['auth_send_code', 'auth_verify_code'])

export const POST: RequestHandler = async ({ locals, request }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }
    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    let body: { action?: unknown; params?: unknown }
    try {
        body = await request.json()
    } catch {
        throw error(400, 'Invalid JSON body')
    }

    const action = typeof body.action === 'string' ? body.action : ''
    const params =
        body.params && typeof body.params === 'object'
            ? (body.params as Record<string, unknown>)
            : {}

    if (!ALLOWED_ACTIONS.has(action)) {
        throw error(
            400,
            `Unsupported action: ${action}. Expected one of: ${[...ALLOWED_ACTIONS].join(', ')}`,
        )
    }

    const config = getConfig()
    const connectorManagerUrl = config.services.connectorManagerUrl

    // Dispatch through connector-manager — it handles connector URL lookup
    // and verifies the action is marked `authenticated: false` in the manifest.
    let actionRes: Response
    try {
        actionRes = await fetch(`${connectorManagerUrl}/connectors/telegram/action`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action, params }),
        })
    } catch (err) {
        logger.error('Failed to reach connector-manager', { err })
        throw error(502, 'Unable to reach connector-manager')
    }

    if (!actionRes.ok) {
        const errBody = await actionRes.text().catch(() => '(no body)')
        logger.error('Connector-manager action dispatch failed', {
            status: actionRes.status,
            body: errBody,
        })
        throw error(actionRes.status, errBody || 'Connector error')
    }

    const result = (await actionRes.json()) as {
        status?: string
        result?: unknown
        error?: string | null
    }

    if (result.error) {
        return json({ ok: false, error: result.error }, { status: 400 })
    }
    return json({ ok: true, result: result.result ?? {} })
}
