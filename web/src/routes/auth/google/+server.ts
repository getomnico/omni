import { redirect } from '@sveltejs/kit'
import { GoogleOAuthService } from '$lib/server/oauth/google'
import { checkRateLimit } from '$lib/server/rateLimit'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ url, getClientAddress }) => {
    try {
        // Apply rate limiting for OAuth requests
        await checkRateLimit(getClientAddress(), 'oauth-initiate', 5, 60) // 5 requests per minute

        // Check if Google OAuth is configured
        if (!GoogleOAuthService.isConfigured()) {
            console.error('Google OAuth is not configured')
            throw redirect(302, '/login?error=oauth_not_configured')
        }

        // Get optional redirect URI from query parameters
        const redirectUri = url.searchParams.get('redirect_uri') || undefined

        // Validate redirect URI if provided
        if (redirectUri) {
            try {
                const redirectUrl = new URL(redirectUri)
                // Only allow same-origin redirects for security
                if (redirectUrl.origin !== url.origin) {
                    throw redirect(302, '/login?error=invalid_redirect')
                }
            } catch {
                throw redirect(302, '/login?error=invalid_redirect')
            }
        }

        // Generate OAuth authorization URL
        const authUrl = await GoogleOAuthService.generateAuthUrl(redirectUri)

        // Redirect to Google OAuth
        throw redirect(302, authUrl)
    } catch (error) {
        console.error('OAuth initiation error:', error)

        // Handle rate limiting errors
        if (error instanceof Error && error.message.includes('Rate limit')) {
            throw redirect(302, '/login?error=rate_limit')
        }

        // Re-throw redirects
        if (error instanceof Response) {
            throw error
        }

        // Handle other errors
        throw redirect(302, '/login?error=oauth_error')
    }
}

// Also handle POST requests for consistency
export const POST: RequestHandler = GET
