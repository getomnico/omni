import type { Handle, HandleServerError } from '@sveltejs/kit'
import { sequence } from '@sveltejs/kit/hooks'
import { redirect } from '@sveltejs/kit'

import * as auth from '$lib/server/auth.js'
import { validateApiKey } from '$lib/server/apiKeys.js'
import { rateLimit } from '$lib/server/rateLimit.js'
import { Logger } from '$lib/server/logger.js'
import { getRequestId, recordHttpRequest, millisecondsToSeconds } from '$lib/server/telemetry.js'

const handleAuth: Handle = async ({ event, resolve }) => {
    // 1. Try API key auth (Authorization: Bearer omni_* or X-API-Key header)
    const authHeader = event.request.headers.get('authorization')
    const xApiKey = event.request.headers.get('x-api-key')
    const apiKeyValue =
        (authHeader?.startsWith('Bearer omni_') ? authHeader.slice(7) : null) || xApiKey

    if (apiKeyValue?.startsWith('omni_')) {
        // Rate limit API key auth attempts per IP (30 attempts per 60s window)
        const ip = event.getClientAddress()
        const rl = await rateLimit(`${ip}:api-key-auth`, 30, 60)
        if (!rl.success) {
            return new Response(JSON.stringify({ error: 'Too many requests' }), {
                status: 429,
                headers: { 'Content-Type': 'application/json' },
            })
        }

        const result = await validateApiKey(apiKeyValue)
        if (result) {
            event.locals.user = result.user
            event.locals.session = null
            event.locals.apiKeyAllowedSources = result.allowedSources
            event.locals.apiKeyScope = result.scope
            return resolve(event)
        }
        // Invalid API key on /api/ routes → 401 immediately
        if (event.url.pathname.startsWith('/api/')) {
            return new Response(JSON.stringify({ error: 'Invalid or expired API key' }), {
                status: 401,
                headers: { 'Content-Type': 'application/json' },
            })
        }
        // For non-API routes (browser), fall through to cookie auth
    }

    // 2. Fall through to cookie-based session auth
    const sessionToken = event.cookies.get(auth.sessionCookieName)

    if (!sessionToken) {
        event.locals.user = null
        event.locals.session = null
        event.locals.apiKeyAllowedSources = null
        event.locals.apiKeyScope = null
        return resolve(event)
    }

    const { session, user } = await auth.validateSessionToken(sessionToken)

    if (session) {
        auth.setSessionTokenCookie(event.cookies, sessionToken, session.expiresAt)
    } else {
        auth.deleteSessionTokenCookie(event.cookies)
    }

    event.locals.user = user
    event.locals.session = session
    event.locals.apiKeyAllowedSources = null // cookie auth = unrestricted
    event.locals.apiKeyScope = null
    return resolve(event)
}

const handlePasswordChange: Handle = async ({ event, resolve }) => {
    const user = event.locals.user

    if (user && user.mustChangePassword) {
        const isChangePasswordRoute = event.url.pathname === '/change-password'
        const isLogoutRoute = event.url.pathname === '/logout'
        const isApiRoute = event.url.pathname.startsWith('/api/')

        if (!isChangePasswordRoute && !isLogoutRoute && !isApiRoute) {
            throw redirect(302, '/change-password')
        }
    }

    return resolve(event)
}

const handleLogging: Handle = async ({ event, resolve }) => {
    // Use trace ID as request ID if available, otherwise generate new one.
    // The Node auto-instrumentation already creates a server span and extracts
    // the incoming traceparent; we just need its trace ID for logging.
    const requestId = getRequestId() || Logger.generateRequestId()
    const logger = new Logger('request').withRequest(requestId, event.locals.user?.id)

    event.locals.requestId = requestId
    event.locals.logger = logger

    const startTime = performance.now()
    const route = event.route.id ?? '/unknown'

    logger.info('Request started', {
        method: event.request.method,
        route,
    })

    let responseStatus: number = 500
    let error: unknown = null
    let response: Response | undefined

    try {
        response = await resolve(event)
        responseStatus = response.status
    } catch (thrown: unknown) {
        // SvelteKit throws redirect(...) and error(...) which are Response-like.
        // Capture the status when available; default to 500 for true errors.
        if (thrown instanceof Response) {
            responseStatus = thrown.status
        } else if (thrown && typeof thrown === 'object' && 'status' in (thrown as object)) {
            responseStatus = (thrown as { status: number }).status
        } else {
            responseStatus = 500
        }
        error = thrown
        // Rethrow after recording so the framework's error handler still fires.
        throw thrown
    } finally {
        const durationMs = performance.now() - startTime
        const durationSecs = millisecondsToSeconds(durationMs)

        if (error === null && response) {
            logger.info('Request completed', {
                method: event.request.method,
                route,
                status: responseStatus,
                duration: durationMs,
            })
        }

        // Record HTTP RED metric with bounded attributes
        // Uses route.id (template) not raw path; never records user/query IDs.
        // Duration is in seconds (OTel standard for histograms).
        recordHttpRequest(event.request.method, route, responseStatus, durationSecs)
    }

    return response!
}

export const handle = sequence(handleLogging, handleAuth, handlePasswordChange)

export const handleError: HandleServerError = ({ error, event }) => {
    const logger = event.locals.logger || new Logger('error')

    logger.error('Unhandled server error', error as Error, {
        method: event.request.method,
        userId: event.locals.user?.id,
        requestId: event.locals.requestId,
    })

    return {
        message: 'Something went wrong',
    }
}
