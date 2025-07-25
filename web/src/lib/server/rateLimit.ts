import type { RequestEvent } from '@sveltejs/kit'

// Simple in-memory rate limiter
// In production, you might want to use Redis for this
const attempts = new Map<string, { count: number; firstAttempt: number }>()

interface RateLimitOptions {
    maxAttempts: number
    windowMs: number
}

const AUTH_RATE_LIMIT: RateLimitOptions = {
    maxAttempts: 5, // 5 attempts
    windowMs: 15 * 60 * 1000, // 15 minutes
}

export function checkRateLimit(
    event: RequestEvent,
    options: RateLimitOptions = AUTH_RATE_LIMIT,
): boolean {
    const clientIp = getClientIp(event)
    const now = Date.now()
    const key = `${clientIp}:${event.url.pathname}`

    const current = attempts.get(key)

    // Clean up old entries
    if (current && now - current.firstAttempt > options.windowMs) {
        attempts.delete(key)
    }

    const existing = attempts.get(key)

    if (!existing) {
        attempts.set(key, { count: 1, firstAttempt: now })
        return true
    }

    if (existing.count >= options.maxAttempts) {
        return false
    }

    existing.count++
    return true
}

export function resetRateLimit(event: RequestEvent): void {
    const clientIp = getClientIp(event)
    const key = `${clientIp}:${event.url.pathname}`
    attempts.delete(key)
}

interface RateLimitResult {
    success: boolean
    remainingAttempts?: number
    resetTime?: number
}

export async function rateLimit(
    key: string,
    maxAttempts: number,
    windowSeconds: number,
): Promise<RateLimitResult> {
    const now = Date.now()
    const windowMs = windowSeconds * 1000

    const current = attempts.get(key)

    // Clean up old entries
    if (current && now - current.firstAttempt > windowMs) {
        attempts.delete(key)
    }

    const existing = attempts.get(key)

    if (!existing) {
        attempts.set(key, { count: 1, firstAttempt: now })
        return {
            success: true,
            remainingAttempts: maxAttempts - 1,
            resetTime: now + windowMs,
        }
    }

    if (existing.count >= maxAttempts) {
        return {
            success: false,
            remainingAttempts: 0,
            resetTime: existing.firstAttempt + windowMs,
        }
    }

    existing.count++
    return {
        success: true,
        remainingAttempts: maxAttempts - existing.count,
        resetTime: existing.firstAttempt + windowMs,
    }
}

export async function applyRateLimit(
    ip: string,
    key: string,
    maxAttempts: number,
    windowSeconds: number,
): Promise<void> {
    const rateLimitKey = `${ip}:${key}`
    const result = await rateLimit(rateLimitKey, maxAttempts, windowSeconds)

    if (!result.success) {
        throw new Error(
            `Rate limit exceeded. Try again at ${new Date(result.resetTime!).toISOString()}`,
        )
    }
}

function getClientIp(event: RequestEvent): string {
    // Check various headers for the real IP
    const forwarded = event.request.headers.get('x-forwarded-for')
    if (forwarded) {
        return forwarded.split(',')[0].trim()
    }

    const realIp = event.request.headers.get('x-real-ip')
    if (realIp) {
        return realIp
    }

    const cfConnectingIp = event.request.headers.get('cf-connecting-ip')
    if (cfConnectingIp) {
        return cfConnectingIp
    }

    // Fallback to a default IP (in development)
    return '127.0.0.1'
}
