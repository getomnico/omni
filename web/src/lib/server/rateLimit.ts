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
