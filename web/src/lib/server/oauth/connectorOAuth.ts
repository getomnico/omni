import { app, getConfig } from '../config'
import { getConnectorConfig } from '../db/connector-configs'
import { OAuthStateManager } from './state'
import type { OAuthError, OAuthTokens } from './types'

/// Mirrors `shared::models::OAuthManifestConfig` (Rust). Pure data: a connector
/// declares this in its manifest and the web app's generic OAuth2 client uses
/// it to drive the standard authorization-code flow.
export interface OAuthManifestConfig {
    provider: string
    auth_endpoint: string
    token_endpoint: string
    userinfo_endpoint: string
    userinfo_email_field: string
    identity_scopes: string[]
    scopes: Record<string, { read: string[]; write: string[] }>
    extra_auth_params: Record<string, string>
    scope_separator: string
    enrich_endpoint?: string | null
}

/// What flow we're driving — encoded into the OAuth state so the single
/// callback route can dispatch correctly.
export type OAuthFlow =
    /// Source admin/personal connect: triggers source creation or attaches
    /// per-user creds to an existing org source.
    | { type: 'connect_source'; sourceTypes: string[]; returnTo?: string }
    /// User attaches per-user write creds to a specific org source.
    | { type: 'user_write'; sourceId: string; returnTo?: string }

export interface ManifestOAuthState {
    user_id?: string
    metadata?: {
        flow: OAuthFlow
        provider: string
        requiredScopes: string[]
        // Granted-scope validation mode: writes require *exact* coverage of
        // requiredScopes; reads/identity don't.
        strictScopeCheck: boolean
    }
}

/// Build the unified callback URL. Stable across all providers and flows so
/// admins register exactly one redirect URI per OAuth client.
export function callbackUrl(): string {
    return `${app.publicUrl}/api/oauth/callback`
}

/// Fetch a connector manifest from connector-manager by source_type. Returns
/// the manifest's oauth block, or null if the connector either isn't
/// registered or doesn't declare an OAuth config.
export async function getOAuthManifestForSourceType(
    sourceType: string,
): Promise<OAuthManifestConfig | null> {
    const cfg = getConfig()
    const resp = await fetch(`${cfg.services.connectorManagerUrl}/connectors`)
    if (!resp.ok) return null
    const body = (await resp.json()) as Array<{
        source_type: string
        manifest?: { oauth?: OAuthManifestConfig | null } | null
    }>
    const entry = body.find((c) => c.source_type === sourceType)
    return entry?.manifest?.oauth ?? null
}

interface ClientCreds {
    clientId: string
    clientSecret: string
}

async function loadClientCreds(provider: string): Promise<ClientCreds | null> {
    const row = await getConnectorConfig(provider)
    if (!row) return null
    const config = row.config as Record<string, string>
    const clientId = config.oauth_client_id
    const clientSecret = config.oauth_client_secret
    if (!clientId || !clientSecret) return null
    return { clientId, clientSecret }
}

export async function isProviderConfigured(provider: string): Promise<boolean> {
    return (await loadClientCreds(provider)) !== null
}

/// Derive the scopes required by a flow against a given source_type.
function scopesForFlow(
    config: OAuthManifestConfig,
    sourceTypes: string[],
    mode: 'read' | 'write',
): string[] {
    const out = new Set<string>(config.identity_scopes)
    for (const t of sourceTypes) {
        const set = config.scopes[t]
        if (!set) continue
        for (const s of set[mode]) out.add(s)
    }
    return [...out]
}

/// Build the authorization URL for a given flow.
export async function generateAuthUrl(args: {
    flow: OAuthFlow
    userId: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    const { flow, userId } = args

    // Pick the manifest based on the source_type(s) involved in this flow.
    const sourceType = flow.type === 'user_write' ? null : flow.sourceTypes[0]
    if (!sourceType && flow.type === 'user_write') {
        throw new Error('user_write flow requires a source')
    }

    let manifestConfig: OAuthManifestConfig | null
    if (flow.type === 'user_write') {
        // Need to look up source's source_type first — caller already did this
        // but we don't pass it through to keep the API tight; do it here.
        throw new Error('user_write flow must be started via generateAuthUrlForSource')
    } else {
        manifestConfig = await getOAuthManifestForSourceType(flow.sourceTypes[0])
    }

    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${sourceType}`)
    }

    const creds = await loadClientCreds(manifestConfig.provider)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    // For `connect_source` we want read scopes (the source will sync); write
    // scopes are only granted by the explicit user_write flow.
    const mode: 'read' | 'write' = 'read'
    const sourceTypes = flow.type === 'connect_source' ? flow.sourceTypes : []
    const requiredScopes = scopesForFlow(manifestConfig, sourceTypes, mode)

    const { stateToken } = await OAuthStateManager.createState(
        manifestConfig.provider,
        callbackUrl(),
        userId,
        {
            flow,
            provider: manifestConfig.provider,
            requiredScopes,
            strictScopeCheck: false,
        },
    )

    return {
        url: buildAuthUrl(manifestConfig, creds, requiredScopes, stateToken),
        requiredScopes,
    }
}

/// Variant for the user-write flow where the caller already has the source's
/// source_type in hand.
export async function generateAuthUrlForUserWrite(args: {
    sourceId: string
    sourceType: string
    userId: string
    returnTo?: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    const manifestConfig = await getOAuthManifestForSourceType(args.sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${args.sourceType}`)
    }
    const creds = await loadClientCreds(manifestConfig.provider)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    const writeScopes = manifestConfig.scopes[args.sourceType]?.write ?? []
    if (writeScopes.length === 0) {
        throw new Error(`No write scopes declared for source_type=${args.sourceType}`)
    }

    // Send identity + write scopes in the auth request. Strict-validate only
    // the write scopes — providers (Google) rewrite identity scope aliases
    // (`email` → `userinfo.email`) so equality on identity scopes is fragile.
    const sentScopes = [...new Set([...manifestConfig.identity_scopes, ...writeScopes])]

    const flow: OAuthFlow = {
        type: 'user_write',
        sourceId: args.sourceId,
        returnTo: args.returnTo,
    }

    const { stateToken } = await OAuthStateManager.createState(
        manifestConfig.provider,
        callbackUrl(),
        args.userId,
        {
            flow,
            provider: manifestConfig.provider,
            requiredScopes: writeScopes,
            strictScopeCheck: true,
        },
    )

    return {
        url: buildAuthUrl(manifestConfig, creds, sentScopes, stateToken),
        requiredScopes: writeScopes,
    }
}

function buildAuthUrl(
    config: OAuthManifestConfig,
    creds: ClientCreds,
    scopes: string[],
    stateToken: string,
): string {
    const params = new URLSearchParams({
        client_id: creds.clientId,
        redirect_uri: callbackUrl(),
        response_type: 'code',
        scope: scopes.join(config.scope_separator),
        state: stateToken,
        ...config.extra_auth_params,
    })
    return `${config.auth_endpoint}?${params.toString()}`
}

export interface ExchangeResult {
    tokens: OAuthTokens
    state: ManifestOAuthState
    config: OAuthManifestConfig
    principalEmail: string
}

/// Exchange an authorization code for tokens, validate state, fetch
/// principal email, and (optionally) call the connector's enrich endpoint.
export async function exchangeCodeAndIdentify(
    code: string,
    stateToken: string,
): Promise<ExchangeResult> {
    const state = (await OAuthStateManager.validateAndConsumeState(
        stateToken,
    )) as ManifestOAuthState | null
    if (!state || !state.metadata) {
        throw new Error('Invalid or expired OAuth state')
    }

    const provider = state.metadata.provider
    const flow = state.metadata.flow
    const config = await manifestForFlow(flow, provider)
    if (!config) {
        throw new Error(`No OAuth manifest for provider=${provider}`)
    }

    const creds = await loadClientCreds(provider)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${provider}`)
    }

    const tokenParams = new URLSearchParams({
        client_id: creds.clientId,
        client_secret: creds.clientSecret,
        code,
        grant_type: 'authorization_code',
        redirect_uri: callbackUrl(),
    })

    const tokenResp = await fetch(config.token_endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: tokenParams.toString(),
    })
    const tokenData = await tokenResp.json()
    if (!tokenResp.ok) {
        const err = tokenData as OAuthError
        throw new Error(`OAuth token exchange failed: ${err.error} - ${err.error_description}`)
    }
    const tokens = tokenData as OAuthTokens

    const userinfoResp = await fetch(config.userinfo_endpoint, {
        headers: { Authorization: `Bearer ${tokens.access_token}` },
    })
    if (!userinfoResp.ok) {
        throw new Error(`Failed to fetch userinfo: ${userinfoResp.status}`)
    }
    const profile = (await userinfoResp.json()) as Record<string, unknown>
    const email = profile[config.userinfo_email_field]
    if (typeof email !== 'string') {
        throw new Error(`userinfo response missing field "${config.userinfo_email_field}"`)
    }

    return { tokens, state, config, principalEmail: email }
}

async function manifestForFlow(
    flow: OAuthFlow,
    provider: string,
): Promise<OAuthManifestConfig | null> {
    // Either flow form has at least one source_type to look up. user_write
    // flows lose their source_type, so we look up by provider via /connectors
    // and find a manifest whose oauth.provider matches.
    if (flow.type === 'connect_source' && flow.sourceTypes.length > 0) {
        return getOAuthManifestForSourceType(flow.sourceTypes[0])
    }
    return getOAuthManifestForProvider(provider)
}

async function getOAuthManifestForProvider(provider: string): Promise<OAuthManifestConfig | null> {
    const cfg = getConfig()
    const resp = await fetch(`${cfg.services.connectorManagerUrl}/connectors`)
    if (!resp.ok) return null
    const body = (await resp.json()) as Array<{
        manifest?: { oauth?: OAuthManifestConfig | null } | null
    }>
    for (const entry of body) {
        const oauth = entry?.manifest?.oauth
        if (oauth && oauth.provider === provider) return oauth
    }
    return null
}
