import type { Handle, HandleServerError } from '@sveltejs/kit'
import { sequence } from '@sveltejs/kit/hooks'
import * as auth from '$lib/server/auth.js'
import { Logger } from '$lib/server/logger.js'
import { initTelemetry, extractTraceContext, getRequestId } from '$lib/server/telemetry.js'

// Initialize OpenTelemetry on module load
initTelemetry()

const handleAuth: Handle = async ({ event, resolve }) => {
    const sessionToken = event.cookies.get(auth.sessionCookieName)

    if (!sessionToken) {
        event.locals.user = null
        event.locals.session = null
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

export const handle = sequence(handleLogging, handleAuth)

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
