import { fail, redirect } from '@sveltejs/kit'
import {
    validateSessionToken,
    setSessionTokenCookie,
    createSession,
    generateSessionToken,
} from '$lib/server/auth.js'
import { sha256 } from '@oslojs/crypto/sha2'
import { encodeHexLowerCase } from '@oslojs/encoding'
import { eq } from 'drizzle-orm'
import { db } from '$lib/server/db/index.js'
import { user } from '$lib/server/db/schema.js'
import { verify } from '@node-rs/argon2'
import type { Actions, PageServerLoad } from './$types.js'

// Rate limiting store (in production, use Redis)
const loginAttempts = new Map<string, { count: number; lastAttempt: number }>()
const RATE_LIMIT_MAX_ATTEMPTS = 5
const RATE_LIMIT_WINDOW = 15 * 60 * 1000 // 15 minutes

function checkRateLimit(ip: string): boolean {
    const now = Date.now()
    const attempts = loginAttempts.get(ip)

    if (!attempts) {
        loginAttempts.set(ip, { count: 1, lastAttempt: now })
        return true
    }

    if (now - attempts.lastAttempt > RATE_LIMIT_WINDOW) {
        loginAttempts.set(ip, { count: 1, lastAttempt: now })
        return true
    }

    if (attempts.count >= RATE_LIMIT_MAX_ATTEMPTS) {
        return false
    }

    attempts.count++
    attempts.lastAttempt = now
    return true
}

export const load: PageServerLoad = async ({ cookies, locals, url }) => {
    if (locals.user) {
        throw redirect(302, '/')
    }

    // Handle OAuth error messages from URL parameters
    const error = url.searchParams.get('error')
    const errorDetails = url.searchParams.get('details')

    if (error) {
        let errorMessage = 'An error occurred during authentication.'

        switch (error) {
            case 'oauth_not_configured':
                errorMessage =
                    'Google Sign-in is not configured. Please contact your administrator.'
                break
            case 'oauth_error':
                errorMessage = errorDetails || 'An error occurred during Google authentication.'
                break
            case 'domain_not_approved':
                errorMessage = errorDetails || 'Your domain is not approved for registration.'
                break
            case 'account_already_linked':
                errorMessage =
                    errorDetails || 'This Google account is already linked to another user.'
                break
            case 'email_mismatch':
                errorMessage = errorDetails || 'Email addresses do not match.'
                break
            case 'rate_limit':
                errorMessage = 'Too many authentication attempts. Please try again later.'
                break
            case 'invalid_redirect':
                errorMessage = 'Invalid redirect URL.'
                break
            case 'invalid_oauth_response':
                errorMessage = 'Invalid OAuth response from Google.'
                break
            case 'authentication_required':
                errorMessage = 'Authentication required.'
                break
            case 'session_expired':
                errorMessage = 'Your session has expired. Please sign in again.'
                break
        }

        return {
            error: errorMessage,
        }
    }

    // Handle success messages
    const welcome = url.searchParams.get('welcome')
    const linked = url.searchParams.get('linked')

    if (welcome) {
        return {
            success: 'Welcome to Clio! Your account has been created successfully.',
        }
    }

    if (linked) {
        return {
            success: `Your ${linked} account has been successfully linked.`,
        }
    }

    return {}
}

export const actions: Actions = {
    default: async ({ request, cookies, getClientAddress }) => {
        const clientIP = getClientAddress()

        if (!checkRateLimit(clientIP)) {
            return fail(429, {
                error: 'Too many login attempts. Please try again in 15 minutes.',
            })
        }

        const formData = await request.formData()
        const email = formData.get('email') as string
        const password = formData.get('password') as string

        if (!email || !password) {
            return fail(400, {
                error: 'Email and password are required.',
                email,
            })
        }

        if (typeof email !== 'string' || typeof password !== 'string') {
            return fail(400, {
                error: 'Invalid form data.',
                email,
            })
        }

        // Email validation
        const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
        if (!emailRegex.test(email)) {
            return fail(400, {
                error: 'Please enter a valid email address.',
                email,
            })
        }

        if (password.length < 6 || password.length > 128) {
            return fail(400, {
                error: 'Password must be between 6 and 128 characters.',
                email,
            })
        }

        try {
            const [foundUser] = await db
                .select()
                .from(user)
                .where(eq(user.email, email.toLowerCase()))

            if (!foundUser) {
                return fail(400, {
                    error: 'Invalid email or password.',
                    email,
                })
            }

            const validPassword = await verify(foundUser.passwordHash, password)
            if (!validPassword) {
                return fail(400, {
                    error: 'Invalid email or password.',
                    email,
                })
            }

            if (!foundUser.isActive) {
                return fail(403, {
                    error: 'Your account has been deactivated. Please contact an administrator.',
                    email,
                })
            }

            // Create session
            const sessionToken = generateSessionToken()
            const session = await createSession(sessionToken, foundUser.id)

            setSessionTokenCookie(cookies, sessionToken, session.expiresAt)

            // Clear rate limiting on successful login
            loginAttempts.delete(clientIP)
        } catch (error) {
            console.error('Login error:', error)
            return fail(500, {
                error: 'An unexpected error occurred. Please try again.',
                email,
            })
        }

        throw redirect(302, '/')
    },
}
