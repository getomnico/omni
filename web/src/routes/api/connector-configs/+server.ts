import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import {
    getAllConnectorConfigsPublic,
    getConnectorConfig,
    upsertConnectorConfig,
} from '$lib/server/db/connector-configs'
import { validateWindshiftServerUrl } from '$lib/server/windshift-server-config'
import { revokeDynamicallyRegisteredClient } from '$lib/server/oauth/connectorOAuth'

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value)
}

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

    const body: unknown = await request.json().catch(() => null)
    if (!isRecord(body) || typeof body.provider !== 'string' || !body.provider.trim()) {
        throw error(400, 'Provider is required')
    }
    if (!isRecord(body.config)) {
        throw error(400, 'Config must be an object')
    }
    const provider = body.provider.trim()
    const config = body.config

    const existing = await getConnectorConfig(provider)
    const existingConfig = (existing?.config ?? {}) as Record<string, unknown>
    const nextConfig = { ...existingConfig, ...config }

    // Replacing a dynamically registered client must not leave the old
    // client active, and the stored identity must not keep fast-path
    // authorization to the revoked client credentials.
    const submittedClientId = config.oauth_client_id
    const clientIdChanged = Boolean(
        typeof submittedClientId === 'string' &&
        submittedClientId.trim() &&
        submittedClientId.trim() !== existingConfig.oauth_client_id,
    )
    const dcrTokenReplaced = Boolean(
        typeof config.oauth_registration_initial_access_token === 'string' &&
        config.oauth_registration_initial_access_token.trim() !== '' &&
        config.oauth_registration_initial_access_token !== '••••••••',
    )
    const hadDynamicClient =
        existingConfig.oauth_dynamic_client_registration === 'true' ||
        (typeof existingConfig.oauth_registration_client_uri === 'string' &&
            typeof existingConfig.oauth_registration_access_token === 'string')
    const hasSubmittedClientCredentials = Boolean(
        clientIdChanged &&
            typeof submittedClientId === 'string' &&
            typeof config.oauth_client_secret === 'string' &&
            config.oauth_client_secret.trim() &&
            config.oauth_client_secret !== '••••••••',
    )
    const replacesDynamicClient = clientIdChanged || dcrTokenReplaced
    if (replacesDynamicClient && hadDynamicClient) {
        if (!(await revokeDynamicallyRegisteredClient(provider, existingConfig))) {
            throw error(409, 'Could not revoke the existing OAuth client; try again later')
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

    // Drop registration metadata after a client replacement. If the admin
    // supplied a new client id and secret, retain those explicit credentials;
    // otherwise the next authorization runs DCR with the new initial access
    // token, or fails closed until a complete client is provided.
    if (replacesDynamicClient) {
        for (const key of [
            'oauth_dynamic_client_registration',
            'oauth_redirect_uri',
            'oauth_registration_endpoint',
            'oauth_registration_client_uri',
            'oauth_registration_access_token',
            'oauth_registration_attempted_at',
        ]) {
            delete nextConfig[key]
        }
        if (!hasSubmittedClientCredentials) {
            // Keep a newly entered client id so the UI can report an
            // incomplete manual configuration rather than silently restoring
            // the previous client. DCR token replacement must clear it to
            // force registration with the new token.
            if (dcrTokenReplaced || !clientIdChanged) {
                delete nextConfig.oauth_client_id
            }
            delete nextConfig.oauth_client_secret
            delete nextConfig.oauth_client_secret_expires_at
            delete nextConfig.oauth_token_endpoint_auth_method
        }
        if (clientIdChanged && !dcrTokenReplaced) {
            delete nextConfig.oauth_registration_initial_access_token
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
