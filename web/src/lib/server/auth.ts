import type { RequestEvent } from '@sveltejs/kit'
import { eq } from 'drizzle-orm'
import { sha256 } from '@oslojs/crypto/sha2'
import { encodeBase64url, encodeHexLowerCase } from '@oslojs/encoding'
import { db } from '$lib/server/db'
import * as table from '$lib/server/db/schema'
import { getRedisClient } from '$lib/server/redis'
import { getConfig } from '$lib/server/config'
import { createLogger } from './logger.js'

const config = getConfig()
const logger = createLogger('auth')
const DAY_IN_MS = 1000 * 60 * 60 * 24
const SESSION_DURATION_MS = DAY_IN_MS * config.session.durationDays

export const sessionCookieName = config.session.cookieName

export function generateSessionToken() {
    const bytes = crypto.getRandomValues(new Uint8Array(18))
    const token = encodeBase64url(bytes)
    return token
}

export async function createSession(token: string, userId: string) {
    const sessionId = encodeHexLowerCase(sha256(new TextEncoder().encode(token)))
    const expiresAt = new Date(Date.now() + SESSION_DURATION_MS)
    const session = {
        id: sessionId,
        userId,
        expiresAt,
    }

    const redis = await getRedisClient()
    await redis.setEx(
        `session:${sessionId}`,
        Math.floor(SESSION_DURATION_MS / 1000),
        JSON.stringify(session),
    )

    return session
}

export async function validateSessionToken(token: string) {
    const sessionId = encodeHexLowerCase(sha256(new TextEncoder().encode(token)))
    const redis = await getRedisClient()

    const sessionData = await redis.get(`session:${sessionId}`)
    if (!sessionData) {
        return { session: null, user: null }
    }

    const session = JSON.parse(sessionData)
    session.expiresAt = new Date(session.expiresAt)

    const sessionExpired = Date.now() >= session.expiresAt.getTime()
    if (sessionExpired) {
        await redis.del(`session:${sessionId}`)
        return { session: null, user: null }
    }

    // Get user data from database
    const [userResult] = await db
        .select({
            id: table.user.id,
            email: table.user.email,
            role: table.user.role,
            isActive: table.user.isActive,
            mustChangePassword: table.user.mustChangePassword,
        })
        .from(table.user)
        .where(eq(table.user.id, session.userId))

    if (!userResult) {
        await redis.del(`session:${sessionId}`)
        return { session: null, user: null }
    }

    // Renew session if it's close to expiry (within 15 days)
    const renewSession = Date.now() >= session.expiresAt.getTime() - DAY_IN_MS * 15
    if (renewSession) {
        session.expiresAt = new Date(Date.now() + SESSION_DURATION_MS)
        await redis.setEx(
            `session:${sessionId}`,
            Math.floor(SESSION_DURATION_MS / 1000),
            JSON.stringify(session),
        )
    }

    return { session, user: userResult }
}

export type SessionValidationResult = Awaited<ReturnType<typeof validateSessionToken>>

export async function invalidateSession(sessionId: string) {
    const redis = await getRedisClient()
    await redis.del(`session:${sessionId}`)
}

export function setSessionTokenCookie(cookies: any, token: string, expiresAt: Date) {
    cookies.set(sessionCookieName, token, {
        expires: expiresAt,
        path: '/',
        secure: false,
    })
}

export function deleteSessionTokenCookie(cookies: any) {
    cookies.delete(sessionCookieName, {
        path: '/',
    })
}

export async function createUserSession(userId: string) {
    try {
        const token = generateSessionToken()
        const session = await createSession(token, userId)

        return {
            success: true,
            session: {
                token,
                ...session,
            },
        }
    } catch (error) {
        logger.error('Error creating session', error, { userId })
        return {
            success: false,
            error: 'Failed to create session',
        }
    }
}

export async function validateSession(sessionToken: string) {
    const result = await validateSessionToken(sessionToken)
    return result.user ? result : null
}
