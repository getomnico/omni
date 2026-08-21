import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'
import { SourceType } from '$lib/types'

/**
 * Invokes an action directly on a connector before a source exists.
 *
 * The transient credential is forwarded to connector-manager's normal /action
 * endpoint and is never stored or returned.
 *
 * SECURITY: Strictly allowlisted — currently only google_drive's
 * discover_folders and validate_shared_drive_access actions with JWT
 * credentials are accepted. Unknown actions, fields, or auth modes fail
 * closed.
 */

const ALLOWED_ACTIONS = new Set(['discover_folders', 'validate_shared_drive_access'])
const ALLOWED_AUTH_MODES = ['domain_wide_delegation', 'service_account_direct'] as const
type GoogleAuthMode = (typeof ALLOWED_AUTH_MODES)[number]

export const POST: RequestHandler = async ({ params: routeParams, request, locals }) => {
    // Admin-only
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }
    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    // Enforce body size limit BEFORE reading — check Content-Length header first.
    const contentLength = request.headers.get('content-length')
    if (contentLength && parseInt(contentLength, 10) > 64 * 1024) {
        throw error(413, 'Request body too large')
    }
    // Read body via streaming reader; stop and cancel once 64KiB is exceeded.
    // Never call arrayBuffer/text first which would buffer the full body.
    let bodyBytes = new Uint8Array(0)
    const reader = request.body?.getReader()
    const MAX_BYTES = 64 * 1024
    if (reader) {
        try {
            while (true) {
                const { done, value } = await reader.read()
                if (done) break
                if (bodyBytes.length + value.length > MAX_BYTES) {
                    await reader.cancel()
                    throw error(413, 'Request body too large')
                }
                const newBody = new Uint8Array(bodyBytes.length + value.length)
                newBody.set(bodyBytes, 0)
                newBody.set(value, bodyBytes.length)
                bodyBytes = newBody
            }
        } catch (err) {
            // If we already threw a 413, re-throw it directly rather than wrapping.
            if (err && typeof err === 'object' && 'status' in err) {
                throw err
            }
            // Otherwise wrap as parse error — empty/null body will land here.
            throw error(400, 'Failed to read request body')
        }
    }
    const text = new TextDecoder().decode(bodyBytes)

    let body: Record<string, unknown>
    try {
        const parsed = JSON.parse(text)
        if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
            throw error(400, 'Request body must be a JSON object')
        }
        body = parsed as Record<string, unknown>
    } catch (err) {
        if (err && typeof err === 'object' && 'status' in err) {
            throw err
        }
        throw error(400, 'Invalid JSON body')
    }

    // ======== STRICT ALLOWLIST ========
    // Reject unknown top-level fields.
    const allowedFields = [
        'action',
        'params',
        'serviceAccountJson',
        'principalEmail',
        'domain',
        'authMode',
    ]
    for (const key of Object.keys(body)) {
        if (!allowedFields.includes(key)) {
            throw error(400, `Unknown field: '${key}'`)
        }
    }

    const sourceType = routeParams.sourceType
    const action = body.action as string | undefined
    const params = body.params as Record<string, unknown> | undefined
    const serviceAccountJson = body.serviceAccountJson as string | undefined
    const principalEmail = body.principalEmail as string | undefined
    const domain = body.domain as string | undefined
    const authMode = (body.authMode as GoogleAuthMode | undefined) ?? 'domain_wide_delegation'

    if (!action) {
        throw error(400, 'Action is required')
    }
    if (!ALLOWED_ACTIONS.has(action)) {
        throw error(400, `Connector action '${action}' is not supported`)
    }
    if (sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, `Connector actions are not supported for source type '${sourceType}'`)
    }
    if (!ALLOWED_AUTH_MODES.includes(authMode)) {
        throw error(400, `Unknown auth mode '${authMode}'`)
    }

    // Validate params by action.
    if (params !== undefined) {
        if (typeof params !== 'object' || params === null || Array.isArray(params)) {
            throw error(400, 'params must be a JSON object')
        }
    }
    if (action === 'discover_folders') {
        const allowedParamFields = ['auth_mode', 'query']
        for (const key of Object.keys(params ?? {})) {
            if (!allowedParamFields.includes(key)) {
                throw error(400, `Unknown discover_folders param: '${key}'`)
            }
        }
        if (params && 'auth_mode' in params) {
            const paramAuthMode = params.auth_mode
            if (
                typeof paramAuthMode !== 'string' ||
                !ALLOWED_AUTH_MODES.includes(paramAuthMode as GoogleAuthMode)
            ) {
                throw error(400, 'discover_folders auth_mode is invalid')
            }
            if (paramAuthMode !== authMode) {
                throw error(400, 'discover_folders auth_mode conflicts with authMode')
            }
        }
        if (!params || !('query' in params)) {
            throw error(400, 'discover_folders query is required')
        }
        const query = params.query
        if (typeof query !== 'string') {
            throw error(400, 'discover_folders query must be a string')
        }
        const queryLength = Array.from(query.trim()).length
        if (queryLength < 2 || queryLength > 200) {
            throw error(400, 'discover_folders query must contain 2–200 characters')
        }
    }
    if (action === 'validate_shared_drive_access') {
        if (!params || typeof params !== 'object' || Array.isArray(params)) {
            throw error(400, 'validate_shared_drive_access requires params')
        }
        const allowedParamFields = ['auth_mode', 'drive_ids']
        for (const key of Object.keys(params)) {
            if (!allowedParamFields.includes(key)) {
                throw error(400, `Unknown validate_shared_drive_access param: '${key}'`)
            }
        }
        if ('auth_mode' in params) {
            const paramAuthMode = params.auth_mode
            if (
                typeof paramAuthMode !== 'string' ||
                !ALLOWED_AUTH_MODES.includes(paramAuthMode as GoogleAuthMode)
            ) {
                throw error(400, 'validate_shared_drive_access auth_mode is invalid')
            }
            if (paramAuthMode !== authMode) {
                throw error(400, 'validate_shared_drive_access auth_mode conflicts with authMode')
            }
        }
        const driveIds = params.drive_ids
        if (
            !Array.isArray(driveIds) ||
            driveIds.length === 0 ||
            !driveIds.every(
                (driveId): driveId is string =>
                    typeof driveId === 'string' && driveId.trim().length > 0,
            )
        ) {
            throw error(400, 'validate_shared_drive_access requires a non-empty drive_ids array')
        }
        if (authMode !== 'service_account_direct') {
            throw error(
                400,
                'validate_shared_drive_access requires authMode=service_account_direct',
            )
        }
    }

    // Transient credentials must be provided for preview (not optional).
    if (!serviceAccountJson) {
        throw error(400, 'serviceAccountJson is required for connector actions')
    }
    // SA-direct requires no principal email or domain (no impersonation).
    if (authMode !== 'service_account_direct' && (!principalEmail || !domain)) {
        throw error(
            400,
            'principalEmail and domain are required for connector actions in domain_wide_delegation mode',
        )
    }

    // Validate types are strings.
    if (typeof serviceAccountJson !== 'string') {
        throw error(400, 'serviceAccountJson must be a string')
    }
    if (principalEmail !== undefined && typeof principalEmail !== 'string') {
        throw error(400, 'principalEmail must be a string')
    }
    if (domain !== undefined && typeof domain !== 'string') {
        throw error(400, 'domain must be a string')
    }

    // Validate service account JSON
    try {
        JSON.parse(serviceAccountJson)
    } catch {
        throw error(400, 'Invalid service account JSON')
    }

    try {
        const config = getConfig()
        const connectorManagerUrl = config.services.connectorManagerUrl

        // Build the transient credential config: SA-direct carries only the
        // drive.readonly scope; DWD carries the domain for impersonation.
        const transientConfig: Record<string, unknown> = {}
        if (authMode === 'service_account_direct') {
            transientConfig.scopes = ['https://www.googleapis.com/auth/drive.readonly']
        } else {
            transientConfig.domain = domain
        }

        // Forward the setup credential through connector-manager's generic transient action mode.
        // auth_mode travels inside params so the connector's typed action params see it.
        const forwardedParams = { ...(params || {}), auth_mode: authMode }
        const response = await fetch(`${connectorManagerUrl}/action`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                source_type: sourceType,
                user_id: locals.user.id,
                action,
                params: forwardedParams,
                transient_credentials: {
                    provider: 'google',
                    auth_type: 'jwt',
                    principal_email: principalEmail ?? null,
                    credentials: {
                        service_account_key: serviceAccountJson,
                    },
                    config: transientConfig,
                },
            }),
            signal: AbortSignal.timeout(30_000),
        })

        if (!response.ok) {
            let errorMessage = `Failed to execute ${action}`
            const contentType = response.headers.get('content-type') || ''
            if (contentType.includes('application/json')) {
                try {
                    const errorBody = await response.json()
                    errorMessage = errorBody.error || errorBody.message || errorMessage
                } catch {
                    // ignore parse errors
                }
            } else {
                errorMessage = (await response.text()) || errorMessage
            }
            logger.error(`Connector action failed`, {
                status: response.status,
                error: errorMessage,
            })
            throw error(response.status, errorMessage)
        }

        const result = await response.json()
        return json(result)
    } catch (err) {
        if (err && typeof err === 'object' && 'status' in err) {
            throw err
        }
        logger.error('Error executing connector action:', err)
        throw error(500, 'Internal server error')
    }
}
