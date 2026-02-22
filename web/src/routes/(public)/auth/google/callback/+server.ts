import { redirect } from '@sveltejs/kit'
import { GoogleOAuthService } from '$lib/server/oauth/google'
import { AccountLinkingService } from '$lib/server/oauth/accountLinking'
import { createSession, generateSessionToken, setSessionTokenCookie } from '$lib/server/auth'
import type { RequestHandler } from './$types'
import { logger } from '$lib/server/logger'

function getErrorRedirect(error: unknown): string {
    if (error instanceof Error) {
        let errorParam = 'oauth_error'
        const errorMessage = error.message

        if (error.message.includes('domain')) {
            errorParam = 'domain_not_approved'
        } else if (error.message.includes('already linked')) {
            errorParam = 'account_already_linked'
        } else if (error.message.includes('email address')) {
            errorParam = 'email_mismatch'
        }

        return `/login?error=${errorParam}&details=${encodeURIComponent(errorMessage)}`
    }

    return '/login?error=oauth_error'
}

export const GET: RequestHandler = async ({ url, cookies }) => {
    // Extract OAuth parameters from the callback URL
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const error = url.searchParams.get('error')

    // Handle OAuth errors
    if (error) {
        logger.error('OAuth callback error:', error)
        const errorDescription = url.searchParams.get('error_description') || 'Unknown OAuth error'
        redirect(302, `/login?error=oauth_error&details=${encodeURIComponent(errorDescription)}`)
    }

    // Validate required parameters
    if (!code || !state) {
        logger.error('Missing required OAuth parameters')
        redirect(302, '/login?error=invalid_oauth_response')
    }

    let successUrl: string

    try {
        // Exchange authorization code for tokens
        const { tokens, state: oauthState } = await GoogleOAuthService.exchangeCodeForTokens(
            code,
            state,
        )

        // Fetch user profile from Google
        const profile = await GoogleOAuthService.fetchUserProfile(tokens.access_token)

        // Authenticate or create user account
        const { user, isNewUser, isLinkedAccount } =
            await AccountLinkingService.authenticateOrCreateUser('google', profile, tokens)

        // Create session for the user
        const token = generateSessionToken()
        const session = await createSession(token, user.id)

        // Set session cookie
        setSessionTokenCookie(cookies, token, session.expiresAt)

        // Determine redirect URL
        let redirectTo = '/'

        if (oauthState.redirect_uri) {
            try {
                const redirectUrl = new URL(oauthState.redirect_uri)
                if (redirectUrl.origin === url.origin) {
                    redirectTo = oauthState.redirect_uri
                }
            } catch {
                // Invalid redirect URI, use default
            }
        }

        const redirectUrl = new URL(redirectTo, url.origin)
        if (isNewUser) {
            redirectUrl.searchParams.set('welcome', 'true')
        }
        if (isLinkedAccount) {
            redirectUrl.searchParams.set('linked', 'google')
        }

        logger.info(`Google OAuth authentication successful for user: ${user.email} (${user.id})`)
        successUrl = redirectUrl.toString()
    } catch (error) {
        logger.error('OAuth callback error:', error)
        redirect(302, getErrorRedirect(error))
    }

    redirect(302, successUrl)
}

// Handle POST requests as well (some OAuth flows might use POST)
export const POST: RequestHandler = GET
