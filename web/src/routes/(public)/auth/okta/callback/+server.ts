import { redirect } from '@sveltejs/kit'
import { getOktaAuthConfig } from '$lib/server/db/auth-providers'
import { app } from '$lib/server/config'
import { OAuthStateManager } from '$lib/server/oauth/state'
import { loadOktaOAuthService } from '$lib/server/oauth/okta'
import { AccountLinkingService } from '$lib/server/oauth/accountLinking'
import { createSession, generateSessionToken, setSessionTokenCookie } from '$lib/server/auth'
import type { RequestHandler } from './$types'
import { logger } from '$lib/server/logger'

function getErrorRedirect(error: unknown): string {
    if (error instanceof Error) {
        if (error.message.includes('domain')) {
            return '/login?error=domain_not_approved'
        } else if (error.message.includes('already linked')) {
            return '/login?error=account_already_linked'
        } else if (error.message.includes('email address')) {
            return '/login?error=email_mismatch'
        }
    }

    return '/login?error=oauth_error'
}

export const GET: RequestHandler = async ({ url, cookies }) => {
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const error = url.searchParams.get('error')

    if (error) {
        const errorDescription = url.searchParams.get('error_description') || 'Unknown OAuth error'
        logger.error('Okta OAuth callback error:', errorDescription)
        redirect(302, '/login?error=oauth_error')
    }

    if (!code || !state) {
        logger.error('Missing required OAuth parameters')
        redirect(302, '/login?error=invalid_oauth_response')
    }

    let successUrl: string

    try {
        // Validate and consume state from Redis
        const oauthState = await OAuthStateManager.validateAndConsumeState(state)
        if (!oauthState) {
            throw new Error('Invalid or expired OAuth state')
        }

        const codeVerifier = oauthState.metadata?.codeVerifier
        if (!codeVerifier) {
            throw new Error('Missing PKCE code verifier')
        }

        const OktaOAuthService = await loadOktaOAuthService()
        if (!OktaOAuthService) {
            redirect(302, '/login?error=okta_not_available')
        }

        const config = await getOktaAuthConfig()
        if (!config || !config.enabled) {
            throw new Error('Okta SSO is not configured')
        }

        const callbackUrl = `${app.publicUrl}/auth/okta/callback`
        const oktaService = new OktaOAuthService(
            {
                oktaDomain: config.oktaDomain,
                clientId: config.clientId,
                clientSecret: config.clientSecret,
            },
            callbackUrl,
        )

        const tokens = await oktaService.exchangeCodeForTokens(code, codeVerifier)
        const profile = await oktaService.fetchUserProfile(tokens.access_token)

        const { user, isNewUser, isLinkedAccount } =
            await AccountLinkingService.authenticateOrCreateUser('okta', profile, tokens)

        const token = generateSessionToken()
        const session = await createSession(token, user.id)
        setSessionTokenCookie(cookies, token, session.expiresAt)

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
            redirectUrl.searchParams.set('linked', 'okta')
        }

        logger.info(`Okta OAuth authentication successful for user: ${user.email} (${user.id})`)
        successUrl = redirectUrl.toString()
    } catch (error) {
        logger.error('Okta OAuth callback error:', error)
        redirect(302, getErrorRedirect(error))
    }

    redirect(302, successUrl)
}

export const POST: RequestHandler = GET
