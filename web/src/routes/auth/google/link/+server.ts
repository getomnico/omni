import { redirect } from '@sveltejs/kit'
import { GoogleOAuthService } from '$lib/server/oauth/google'
import { AccountLinkingService } from '$lib/server/oauth/accountLinking'
import { applyRateLimit } from '$lib/server/rateLimit'
import { validateSession } from '$lib/server/auth'
import { session } from '$lib/server/config'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ url, cookies, getClientAddress }) => {
    try {
        // Apply rate limiting for OAuth linking requests
        await applyRateLimit(getClientAddress(), 'oauth-link', 5, 60) // 5 requests per minute

        // Check if user is authenticated
        const sessionId = cookies.get(session.cookieName)
        if (!sessionId) {
            throw redirect(302, '/login?error=authentication_required')
        }

        const userSession = await validateSession(sessionId)
        if (!userSession) {
            // Clear invalid session cookie
            cookies.delete(session.cookieName, { path: '/' })
            throw redirect(302, '/login?error=session_expired')
        }

        // Check if Google OAuth is configured
        if (!GoogleOAuthService.isConfigured()) {
            console.error('Google OAuth is not configured')
            throw redirect(302, '/settings/integrations?error=oauth_not_configured')
        }

        // Get optional redirect URI from query parameters (default to settings page)
        const redirectUri = url.searchParams.get('redirect_uri') || '/settings/integrations'

        // Validate redirect URI
        try {
            const redirectUrl = new URL(redirectUri, url.origin)
            // Only allow same-origin redirects for security
            if (redirectUrl.origin !== url.origin) {
                throw redirect(302, '/settings/integrations?error=invalid_redirect')
            }
        } catch {
            throw redirect(302, '/settings/integrations?error=invalid_redirect')
        }

        // Generate OAuth authorization URL with user ID for account linking
        const authUrl = await GoogleOAuthService.generateAuthUrl(redirectUri, userSession.userId)

        // Redirect to Google OAuth
        throw redirect(302, authUrl)
    } catch (error) {
        console.error('OAuth link initiation error:', error)

        // Handle rate limiting errors
        if (error instanceof Error && error.message.includes('Rate limit')) {
            throw redirect(302, '/settings/integrations?error=rate_limit')
        }

        // Re-throw redirects
        if (error instanceof Response) {
            throw error
        }

        // Handle other errors
        throw redirect(302, '/settings/integrations?error=oauth_error')
    }
}

export const POST: RequestHandler = GET
