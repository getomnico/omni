import { redirect } from '@sveltejs/kit'
import { getEntraAuthConfig } from '$lib/server/db/auth-providers'
import { app } from '$lib/server/config'
import { OAuthStateManager } from '$lib/server/oauth/state'
import { loadEntraOAuthService } from '$lib/server/oauth/entra'
import type { RequestHandler } from './$types'
import { logger } from '$lib/server/logger'

export const GET: RequestHandler = async ({ url }) => {
    let authUrl: string
    try {
        const config = await getEntraAuthConfig()
        if (!config || !config.enabled) {
            logger.error('Entra SSO is not configured')
            throw redirect(302, '/login?error=oauth_not_configured')
        }

        const EntraOAuthService = await loadEntraOAuthService()
        if (!EntraOAuthService) {
            throw redirect(302, '/login?error=entra_not_available')
        }

        const redirectUri = url.searchParams.get('redirect_uri') || undefined

        if (redirectUri) {
            try {
                const redirectUrl = new URL(redirectUri)
                if (redirectUrl.origin !== url.origin) {
                    throw redirect(302, '/login?error=invalid_redirect')
                }
            } catch {
                throw redirect(302, '/login?error=invalid_redirect')
            }
        }

        const callbackUrl = `${app.publicUrl}/auth/entra/callback`
        const entraService = new EntraOAuthService(
            {
                tenant: config.tenant,
                clientId: config.clientId,
                clientSecret: config.clientSecret,
            },
            callbackUrl,
        )

        const {
            url: entraAuthUrl,
            state,
            codeVerifier,
        } = await entraService.createAuthorizationURL()

        // Store state + codeVerifier in Redis
        const { stateToken } = await OAuthStateManager.createState(
            'entra',
            redirectUri,
            undefined,
            {
                codeVerifier,
                entraState: state,
            },
        )

        // Swap our state token into the auth URL
        const authUrlObj = new URL(entraAuthUrl)
        authUrlObj.searchParams.set('state', stateToken)
        authUrl = authUrlObj.toString()
    } catch (error) {
        logger.error('Entra OAuth initiation error:', error)

        if (error instanceof Response) {
            throw error
        }

        throw redirect(302, '/login?error=oauth_error')
    }

    logger.info('Redirecting to Entra')
    throw redirect(302, authUrl)
}

export const POST: RequestHandler = GET
