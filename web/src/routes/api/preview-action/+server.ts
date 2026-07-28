import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'
import { SourceType } from '$lib/types'

/**
 * POST /api/preview-action
 *
 * Admin-only endpoint that forwards a transient credential (not yet persisted) to
 * the connector for preview/discovery actions such as listing shared drives and
 * top-level folders. The credential is never stored or returned; it flows straight
 * to the connector-manager's /action-preview endpoint and then to the connector.
 *
 * Request body size is limited to 64KB at the web boundary.
 *
 * SECURITY: Strictly allowlisted — only action='discover_folders',
 * sourceType='google_drive', and JWT credentials are accepted.
 *
 * Request body:
 * {
 *   sourceType: 'google_drive',
 *   action: 'discover_folders',
 *   params: {},
 *   serviceAccountJson: string,   // full service-account JSON key
 *   principalEmail: string,       // delegated admin email
 *   domain: string                // Google Workspace domain
 * }
 */
export const POST: RequestHandler = async ({ request, locals }) => {
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
        'sourceType',
        'action',
        'params',
        'serviceAccountJson',
        'principalEmail',
        'domain',
    ]
    for (const key of Object.keys(body)) {
        if (!allowedFields.includes(key)) {
            throw error(400, `Unknown field: '${key}'`)
        }
    }

    const sourceType = body.sourceType as string | undefined
    const action = body.action as string | undefined
    const params = body.params as Record<string, unknown> | undefined
    const serviceAccountJson = body.serviceAccountJson as string | undefined
    const principalEmail = body.principalEmail as string | undefined
    const domain = body.domain as string | undefined

    if (!action) {
        throw error(400, 'Action is required')
    }
    if (action !== 'discover_folders') {
        throw error(400, `Preview action '${action}' is not supported`)
    }
    if (!sourceType || sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, 'Preview action only supports source_type: google_drive')
    }

    // Only allow empty params object for discover_folders.
    if (params !== undefined) {
        if (typeof params !== 'object' || params === null || Array.isArray(params)) {
            throw error(400, 'params must be a JSON object')
        }
        if (Object.keys(params).length > 0) {
            throw error(400, 'discover_folders does not accept any params')
        }
    }

    // Transient credentials must be provided for preview (not optional).
    if (!serviceAccountJson || !principalEmail || !domain) {
        throw error(400, 'serviceAccountJson, principalEmail, and domain are required for preview')
    }

    // Validate types are strings.
    if (typeof serviceAccountJson !== 'string') {
        throw error(400, 'serviceAccountJson must be a string')
    }
    if (typeof principalEmail !== 'string') {
        throw error(400, 'principalEmail must be a string')
    }
    if (typeof domain !== 'string') {
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

        // Build a transient ServiceCredential payload for the connector-manager
        const transientCredential = {
            id: 'preview',
            source_id: 'preview',
            user_id: null as string | null,
            provider: 'google',
            auth_type: 'jwt',
            principal_email: principalEmail,
            credentials: {
                service_account_key: serviceAccountJson,
            },
            config: {
                domain: domain,
            },
            expires_at: null,
            last_validated_at: null,
            created_at: new Date().toISOString(),
            updated_at: new Date().toISOString(),
        }

        const response = await fetch(`${connectorManagerUrl}/action-preview`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                source_type: 'google_drive',
                source_id: null,
                action: 'discover_folders',
                params: params || {},
                credentials: transientCredential,
            }),
            // Limit connector-manager communication too
            signal: AbortSignal.timeout(30_000),
        })

        if (!response.ok) {
            let errorMessage = 'Failed to discover folders'
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
            logger.error(`Preview action failed`, {
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
        logger.error('Error executing preview action:', err)
        throw error(500, 'Internal server error')
    }
}
