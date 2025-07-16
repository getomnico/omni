import { redirect } from '@sveltejs/kit'
import { GoogleOAuthService } from '$lib/server/oauth/google'
import { AccountLinkingService } from '$lib/server/oauth/accountLinking'
import { applyRateLimit } from '$lib/server/rateLimit'
import { createSession } from '$lib/server/auth'
import { session } from '$lib/server/config'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ url, cookies, getClientAddress }) => {
    try {
        // Apply rate limiting for OAuth callbacks
        await applyRateLimit(getClientAddress(), 'oauth-callback', 10, 60) // 10 requests per minute

        // Extract OAuth parameters from the callback URL
        const code = url.searchParams.get('code')
        const state = url.searchParams.get('state')
        const error = url.searchParams.get('error')

        // Handle OAuth errors
        if (error) {
            console.error('OAuth callback error:', error)
            const errorDescription =
                url.searchParams.get('error_description') || 'Unknown OAuth error'
            throw redirect(
                302,
                `/login?error=oauth_error&details=${encodeURIComponent(errorDescription)}`,
            )
        }

        // Validate required parameters
        if (!code || !state) {
            console.error('Missing required OAuth parameters')
            throw redirect(302, '/login?error=invalid_oauth_response')
        }

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
        const sessionId = await createSession(user.id, user.role)

        // Set session cookie
        cookies.set(session.cookieName, sessionId, {
            path: '/',
            httpOnly: true,
            secure: true,
            sameSite: 'lax',
            maxAge: session.durationDays * 24 * 60 * 60, // Convert days to seconds
        })

        // Determine redirect URL
        let redirectTo = '/'

        if (oauthState.redirect_uri) {
            try {
                const redirectUrl = new URL(oauthState.redirect_uri)
                // Only allow same-origin redirects for security
                if (redirectUrl.origin === url.origin) {
                    redirectTo = oauthState.redirect_uri
                }
            } catch {
                // Invalid redirect URI, use default
            }
        }

        // Add success parameters to redirect URL
        const redirectUrl = new URL(redirectTo, url.origin)
        if (isNewUser) {
            redirectUrl.searchParams.set('welcome', 'true')
        }
        if (isLinkedAccount) {
            redirectUrl.searchParams.set('linked', 'google')
        }

        console.log(`Google OAuth authentication successful for user: ${user.email} (${user.id})`)
        throw redirect(302, redirectUrl.toString())
    } catch (error) {
        console.error('OAuth callback error:', error)

        // Handle rate limiting errors
        if (error instanceof Error && error.message.includes('Rate limit')) {
            throw redirect(302, '/login?error=rate_limit')
        }

        // Re-throw redirects
        if (error instanceof Response) {
            throw error
        }

        // Handle authentication errors
        if (error instanceof Error) {
            let errorParam = 'oauth_error'
            let errorMessage = error.message

            if (error.message.includes('domain')) {
                errorParam = 'domain_not_approved'
            } else if (error.message.includes('already linked')) {
                errorParam = 'account_already_linked'
            } else if (error.message.includes('email address')) {
                errorParam = 'email_mismatch'
            }

            throw redirect(
                302,
                `/login?error=${errorParam}&details=${encodeURIComponent(errorMessage)}`,
            )
        }

        // Generic error fallback
        throw redirect(302, '/login?error=oauth_error')
    }
}

// Handle POST requests as well (some OAuth flows might use POST)
export const POST: RequestHandler = GET
