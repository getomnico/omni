import { redirect } from '@sveltejs/kit'
import { GoogleOAuthService } from '$lib/server/oauth/google'
import type { RequestHandler } from './$types'
import { logger } from '$lib/server/logger'

export const GET: RequestHandler = async ({ url }) => {
    let authUrl: string
    try {
        // Check if Google OAuth is configured
        if (!(await GoogleOAuthService.isConfigured())) {
            logger.error('Google OAuth is not configured')
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
        authUrl = await GoogleOAuthService.generateAuthUrl(redirectUri)
    } catch (error) {
        logger.error('OAuth initiation error:', error)

        // Re-throw redirects
        if (error instanceof Response) {
            throw error
        }

        // Handle other errors
        throw redirect(302, '/login?error=oauth_error')
    }

    // Redirect to Google OAuth
    logger.info('Redirecting to Google:', authUrl)
    throw redirect(302, authUrl)
}

// Also handle POST requests for consistency
export const POST: RequestHandler = GET
