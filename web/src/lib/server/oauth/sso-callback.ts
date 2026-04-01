import { redirect } from '@sveltejs/kit'
import type { Cookies } from '@sveltejs/kit'
import { app } from '$lib/server/config'
import { OAuthStateManager } from '$lib/server/oauth/state'
import { AccountLinkingService } from '$lib/server/oauth/accountLinking'
import { createSession, generateSessionToken, setSessionTokenCookie } from '$lib/server/auth'
import { logger } from '$lib/server/logger'

export interface SsoCallbackOptions {
    provider: string
    loadConfig: () => Promise<{ enabled: boolean; [key: string]: any } | null>
    loadService: () => Promise<any | null>
    createService: (ServiceClass: any, config: any, callbackUrl: string) => any
    callbackPath: string
}

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

export async function handleSsoCallback(
    url: URL,
    cookies: Cookies,
    options: SsoCallbackOptions,
): Promise<never> {
    const { provider, loadConfig, loadService, createService, callbackPath } = options

    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const error = url.searchParams.get('error')

    if (error) {
        const errorDescription = url.searchParams.get('error_description') || 'Unknown OAuth error'
        logger.error(`${provider} OAuth callback error:`, errorDescription)
        redirect(302, '/login?error=oauth_error')
    }

    if (!code || !state) {
        logger.error('Missing required OAuth parameters')
        redirect(302, '/login?error=invalid_oauth_response')
    }

    let successUrl: string

    try {
        const oauthState = await OAuthStateManager.validateAndConsumeState(state)
        if (!oauthState) {
            throw new Error('Invalid or expired OAuth state')
        }

        const codeVerifier = oauthState.metadata?.codeVerifier
        if (!codeVerifier) {
            throw new Error('Missing PKCE code verifier')
        }

        const ServiceClass = await loadService()
        if (!ServiceClass) {
            redirect(302, `/login?error=${provider}_not_available`)
        }

        const config = await loadConfig()
        if (!config || !config.enabled) {
            throw new Error(`${provider} SSO is not configured`)
        }

        const callbackUrl = `${app.publicUrl}${callbackPath}`
        const service = createService(ServiceClass, config, callbackUrl)

        const tokens = await service.exchangeCodeForTokens(code, codeVerifier)
        const profile = await service.fetchUserProfile(tokens.access_token)

        const { user, isNewUser, isLinkedAccount } =
            await AccountLinkingService.authenticateOrCreateUser(provider, profile, tokens)

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
            redirectUrl.searchParams.set('linked', provider)
        }

        logger.info(
            `${provider} OAuth authentication successful for user: ${user.email} (${user.id})`,
        )
        successUrl = redirectUrl.toString()
    } catch (error) {
        logger.error(`${provider} OAuth callback error:`, error)
        redirect(302, getErrorRedirect(error))
    }

    redirect(302, successUrl)
}
