import { fail, redirect } from '@sveltejs/kit'
import {
    validateSessionToken,
    setSessionTokenCookie,
    createSession,
    generateSessionToken,
} from '$lib/server/auth.js'
import { userRepository } from '$lib/server/db/users'
import { SystemFlags } from '$lib/server/system-flags'
import {
    getGoogleAuthConfig,
    getOktaAuthConfig,
    getEntraAuthConfig,
    isPasswordAuthEnabled,
} from '$lib/server/db/auth-providers'
import { loadOktaOAuthService } from '$lib/server/oauth/okta'
import { loadEntraOAuthService } from '$lib/server/oauth/entra'
import { verify } from '@node-rs/argon2'
import type { Actions, PageServerLoad } from './$types.js'

export const load: PageServerLoad = async ({ cookies, locals, url }) => {
    if (locals.user) {
        throw redirect(302, '/')
    }

    // Check if this is a first-time setup (system not initialized)
    const isInitialized = await SystemFlags.isInitialized()
    if (!isInitialized) {
        // System not initialized, redirect to signup for initial admin creation
        throw redirect(302, '/signup')
    }

    // Check if Google Auth is enabled
    const googleConfig = await getGoogleAuthConfig()
    const googleAuthEnabled = googleConfig?.enabled ?? false

    // Check if Okta SSO is available and enabled
    const oktaConfig = await getOktaAuthConfig()
    const oktaAuthEnabled =
        (oktaConfig?.enabled && (await loadOktaOAuthService()) !== null) ?? false

    // Check if Entra SSO is available and enabled
    const entraConfig = await getEntraAuthConfig()
    const entraAuthEnabled =
        (entraConfig?.enabled && (await loadEntraOAuthService()) !== null) ?? false
    const passwordAuthEnabled = await isPasswordAuthEnabled()

    // Handle OAuth error messages from URL parameters
    const error = url.searchParams.get('error')

    if (error) {
        let errorMessage =
            'An error occurred during authentication. Please try again or contact support.'

        switch (error) {
            case 'oauth_not_configured':
                errorMessage =
                    'Single sign-on is not configured. Please contact your administrator.'
                break
            case 'oauth_error':
                errorMessage =
                    'Something went wrong during authentication. Please try again or contact support.'
                break
            case 'domain_not_approved':
                errorMessage =
                    'Your domain is not approved for registration. Please contact your administrator.'
                break
            case 'account_already_linked':
                errorMessage =
                    'This account is already linked to another user. Please contact support.'
                break
            case 'email_mismatch':
                errorMessage = 'Email addresses do not match. Please contact support.'
                break
            case 'rate_limit':
                errorMessage = 'Too many authentication attempts. Please try again later.'
                break
            case 'invalid_redirect':
                errorMessage = 'Something went wrong. Please try signing in again.'
                break
            case 'invalid_oauth_response':
                errorMessage = 'Something went wrong during authentication. Please try again.'
                break
            case 'okta_not_available':
                errorMessage = 'Okta SSO is not available. Please contact your administrator.'
                break
            case 'entra_not_available':
                errorMessage =
                    'Microsoft Entra ID SSO is not available. Please contact your administrator.'
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
            googleAuthEnabled,
            oktaAuthEnabled,
            entraAuthEnabled,
            passwordAuthEnabled,
        }
    }

    // Handle success messages
    const welcome = url.searchParams.get('welcome')
    const linked = url.searchParams.get('linked')

    if (welcome) {
        return {
            success: 'Welcome to Omni! Your account has been created successfully.',
            googleAuthEnabled,
            oktaAuthEnabled,
            entraAuthEnabled,
            passwordAuthEnabled,
        }
    }

    if (linked) {
        return {
            success: `Your ${linked} account has been successfully linked.`,
            googleAuthEnabled,
            oktaAuthEnabled,
            entraAuthEnabled,
            passwordAuthEnabled,
        }
    }

    return { googleAuthEnabled, oktaAuthEnabled, entraAuthEnabled, passwordAuthEnabled }
}

export const actions: Actions = {
    default: async ({ request, cookies }) => {
        const passwordEnabled = await isPasswordAuthEnabled()
        if (!passwordEnabled) {
            return fail(403, {
                error: 'Password authentication is disabled. Please use an alternative sign-in method.',
                email: '',
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
            const foundUser = await userRepository.findByEmail(email)

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
