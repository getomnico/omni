import { redirect } from '@sveltejs/kit'
import { UserOAuthCredentialsService } from '$lib/server/oauth/userCredentials'
import { applyRateLimit } from '$lib/server/rateLimit'
import { validateSession } from '$lib/server/auth'
import { session } from '$lib/server/config'
import type { RequestHandler } from './$types'

export const POST: RequestHandler = async ({ url, cookies, getClientAddress }) => {
    try {
        // Apply rate limiting for OAuth unlinking requests
        await applyRateLimit(getClientAddress(), 'oauth-unlink', 5, 60) // 5 requests per minute

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

        // Get provider user ID from request body or URL params
        const formData = await url.searchParams
        const providerUserId = formData.get('provider_user_id')

        if (!providerUserId) {
            throw redirect(302, '/settings/integrations?error=missing_provider_user_id')
        }

        // Remove the OAuth credentials
        await UserOAuthCredentialsService.removeCredentials(
            userSession.userId,
            'google',
            providerUserId,
        )

        console.log(`Google OAuth credentials removed for user: ${userSession.userId}`)

        // Redirect back to settings with success message
        throw redirect(302, '/settings/integrations?success=google_unlinked')
    } catch (error) {
        console.error('OAuth unlink error:', error)

        // Handle rate limiting errors
        if (error instanceof Error && error.message.includes('Rate limit')) {
            throw redirect(302, '/settings/integrations?error=rate_limit')
        }

        // Re-throw redirects
        if (error instanceof Response) {
            throw error
        }

        // Handle other errors
        throw redirect(302, '/settings/integrations?error=oauth_unlink_error')
    }
}

// Only allow POST requests for security
export const GET: RequestHandler = async () => {
    throw redirect(302, '/settings/integrations')
}
