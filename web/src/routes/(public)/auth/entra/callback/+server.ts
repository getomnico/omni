import { getEntraAuthConfig } from '$lib/server/db/auth-providers'
import { loadEntraOAuthService } from '$lib/server/oauth/entra'
import { handleSsoCallback } from '$lib/server/oauth/sso-callback'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ url, cookies }) => {
    await handleSsoCallback(url, cookies, {
        provider: 'entra',
        loadConfig: getEntraAuthConfig,
        loadService: loadEntraOAuthService,
        createService: (ServiceClass, config, callbackUrl) =>
            new ServiceClass(
                {
                    tenant: config.tenant,
                    clientId: config.clientId,
                    clientSecret: config.clientSecret,
                },
                callbackUrl,
            ),
        callbackPath: '/auth/entra/callback',
    })
}

export const POST: RequestHandler = GET
