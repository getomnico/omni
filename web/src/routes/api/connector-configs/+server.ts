import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import {
    getAllConnectorConfigsPublic,
    getConnectorConfig,
    upsertConnectorConfig,
} from '$lib/server/db/connector-configs'
import { validateWindshiftServerUrl } from '$lib/server/windshift-server-config'
import { revokeDynamicallyRegisteredClient } from '$lib/server/oauth/connectorOAuth'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const configs = await getAllConnectorConfigsPublic()
    return json(configs)
}

export const POST: RequestHandler = async ({ locals, request }) => {
    if (!locals.user || locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const body = await request.json()
    const { provider, config } = body

    if (!provider || !config) {
        throw error(400, 'Missing provider or config')
    }

    const existing = await getConnectorConfig(provider)
    const existingConfig = (existing?.config ?? {}) as Record<string, unknown>
    const nextConfig = { ...existingConfig, ...config }

    // Replacing a source's dynamically registered client must not leave the
    // old client active in Salesforce, and the stored identity must not keep
    // fast-path authorization to the revoked client_id/client_secret pair.
    const isSalesforceSourceConfig =
        typeof provider === 'string' && provider.startsWith('salesforce:')
    const submittedClientId = config.oauth_client_id
    const clientIdChanged = Boolean(
        isSalesforceSourceConfig &&
        typeof submittedClientId === 'string' &&
        submittedClientId.trim() &&
        submittedClientId.trim() !== existingConfig.oauth_client_id,
    )
    const dcrTokenReplaced = Boolean(
        isSalesforceSourceConfig &&
        typeof config.oauth_registration_initial_access_token === 'string' &&
        config.oauth_registration_initial_access_token.trim() !== '' &&
        config.oauth_registration_initial_access_token !== '••••••••',
    )
    const hadDynamicClient = existingConfig.oauth_dynamic_client_registration === 'true'
    const replacesDynamicClient = clientIdChanged || dcrTokenReplaced
    if (replacesDynamicClient && hadDynamicClient) {
        if (!(await revokeDynamicallyRegisteredClient(provider, existingConfig))) {
            throw error(
                409,
                'Could not revoke the existing Salesforce OAuth client; try again later',
            )
        }
    }

    for (const secretKey of [
        'oauth_client_secret',
        'oauth_registration_initial_access_token',
        'oauth_registration_access_token',
    ]) {
        const submitted = config[secretKey]
        if (
            (!submitted || submitted === '••••••••') &&
            typeof existingConfig[secretKey] === 'string'
        ) {
            nextConfig[secretKey] = existingConfig[secretKey]
        }
    }

    // The old dynamic client was just revoked remotely. Drop its full
    // identity and management metadata (the secret-preservation loop above
    // must not bring the revoked client_secret back). The next authorization
    // then runs DCR with the new initial access token, or fails closed as
    // "not configured" until the admin provides a complete client.
    if (replacesDynamicClient && hadDynamicClient) {
        for (const key of [
            'oauth_client_id',
            'oauth_client_secret',
            'oauth_client_secret_expires_at',
            'oauth_token_endpoint_auth_method',
            'oauth_redirect_uri',
            'oauth_dynamic_client_registration',
            'oauth_registration_endpoint',
            'oauth_registration_client_uri',
            'oauth_registration_access_token',
            'oauth_registration_attempted_at',
        ]) {
            delete nextConfig[key]
        }
    }

    // Windshift's public URL is admin-entered and fetched by the server (OAuth
    // endpoints, MCP) — enforce the same SSRF policy as remote MCP sources.
    // The internal route is env-only (WINDSHIFT_INTERNAL_BASE_URL); it is no
    // longer UI/API-configurable, so reject it here and drop stale values.
    if (provider === 'windshift') {
        const baseUrl = nextConfig.base_url
        if (typeof baseUrl !== 'string' || !baseUrl.trim()) {
            throw error(400, 'Windshift URL is required')
        }
        delete nextConfig.internal_base_url
        try {
            await validateWindshiftServerUrl('Windshift URL', baseUrl)
        } catch (err) {
            throw error(
                400,
                err instanceof Error ? err.message : 'Windshift server URL is not allowed',
            )
        }
    }

    const result = await upsertConnectorConfig(provider, nextConfig, locals.user.id)
    return json({ provider: result.provider, updatedAt: result.updatedAt })
}
