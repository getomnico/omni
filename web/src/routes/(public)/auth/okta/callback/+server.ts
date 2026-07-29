import { getOktaAuthConfig } from '$lib/server/db/auth-providers'
import { loadOktaOAuthService } from '$lib/server/oauth/okta'
import { handleSsoCallback } from '$lib/server/oauth/sso-callback'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ url, cookies, locals }) => {
    return handleSsoCallback(url, cookies, {
        provider: 'okta',
        loadConfig: getOktaAuthConfig,
        loadService: loadOktaOAuthService,
        createService: (ServiceClass, config, callbackUrl) =>
            new ServiceClass(
                {
                    oktaDomain: config.oktaDomain,
                    clientId: config.clientId,
                    clientSecret: config.clientSecret,
                },
                callbackUrl,
            ),
        callbackPath: '/auth/okta/callback',
        logger: locals.logger,
    })
}

export const POST: RequestHandler = GET
