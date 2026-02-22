// Simple in-memory rate limiter
// In production, you might want to use Redis for this
const attempts = new Map<string, { count: number; firstAttempt: number }>()

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
