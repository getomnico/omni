import type { Handle, HandleServerError } from '@sveltejs/kit'
import { sequence } from '@sveltejs/kit/hooks'
import { redirect } from '@sveltejs/kit'
import * as auth from '$lib/server/auth.js'
import { validateApiKey } from '$lib/server/apiKeys.js'
import { rateLimit } from '$lib/server/rateLimit.js'
import { Logger } from '$lib/server/logger.js'
import { initTelemetry, extractTraceContext, getRequestId } from '$lib/server/telemetry.js'
import { drizzle } from 'drizzle-orm/postgres-js'
import * as schema from '$lib/server/db/schema.js'
import { rlsClient } from '$lib/server/db/index.js'

// Initialize OpenTelemetry on module load
initTelemetry()

const handleAuth: Handle = async ({ event, resolve }) => {
    let userId: string | null = null
    let reserved: Awaited<ReturnType<typeof rlsClient.reserve>> | null = null

    try {
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
                userId = result.user.id
            }
            // Invalid API key on /api/ routes → 401 immediately
            if (event.url.pathname.startsWith('/api/') && !result) {
                return new Response(JSON.stringify({ error: 'Invalid or expired API key' }), {
                    status: 401,
                    headers: { 'Content-Type': 'application/json' },
                })
            }
            // For non-API routes (browser), fall through to cookie auth
        }

        // 2. Fall through to cookie-based session auth
        if (!userId) {
            const sessionToken = event.cookies.get(auth.sessionCookieName)

            if (!sessionToken) {
                event.locals.user = null
                event.locals.session = null
                event.locals.apiKeyAllowedSources = null
                event.locals.apiKeyScope = null
            } else {
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
                userId = user?.id ?? null
            }
        }

        // 3. Set up RLS connection with user context. set_config() (instead of
        //    SET) lets us pass the user ID as a bind parameter — SET cannot be
        //    parameterised in Postgres.
        if (userId) {
            reserved = await rlsClient.reserve()
            await reserved`SELECT set_config('app.current_user_id', ${userId}, false)`
            event.locals.db = drizzle(reserved, { schema })
        }

        return await resolve(event)
    } finally {
        // 4. Always release the reserved connection back to the pool
        reserved?.release()
    }
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
    // Extract trace context from incoming request headers
    const headers: Record<string, string | undefined> = {}
    event.request.headers.forEach((value, key) => {
        headers[key] = value
    })
    extractTraceContext(headers)

    // Use trace ID as request ID if available, otherwise generate new one
    const requestId = getRequestId() || Logger.generateRequestId()
    const logger = new Logger('request').withRequest(requestId, event.locals.user?.id)

    event.locals.requestId = requestId
    event.locals.logger = logger

    const startTime = Date.now()

    logger.info('Request started', {
        method: event.request.method,
        url: event.url.pathname + event.url.search,
        userAgent: event.request.headers.get('user-agent'),
        ip: event.getClientAddress(),
        userId: event.locals.user?.id,
        userEmail: event.locals.user?.email,
    })

    const response = await resolve(event)

    const duration = Date.now() - startTime

    logger.info('Request completed', {
        method: event.request.method,
        url: event.url.pathname + event.url.search,
        status: response.status,
        duration,
        userId: event.locals.user?.id,
    })

    return response
}

export const handle = sequence(handleLogging, handleAuth, handlePasswordChange)

export const handleError: HandleServerError = ({ error, event }) => {
    const logger = event.locals.logger || new Logger('error')

    logger.error('Unhandled server error', error as Error, {
        url: event.url.pathname + event.url.search,
        method: event.request.method,
        userId: event.locals.user?.id,
        requestId: event.locals.requestId,
    })

    return {
        message: 'Something went wrong',
    }
}
