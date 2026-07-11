import { createHash, randomBytes } from 'crypto'
import { app, getConfig } from '../config'
import { getConnectorConfig, upsertConnectorConfig } from '../db/connector-configs'
import { createLogger } from '../logger'
import { OAuthStateManager } from './state'
import type { OAuthError, OAuthTokens } from './types'

export type OAuthTokenEndpointAuthMethod = 'client_secret_post' | 'client_secret_basic' | 'none'

const logger = createLogger('connector-oauth')

function isOAuthTokenEndpointAuthMethod(value: unknown): value is OAuthTokenEndpointAuthMethod {
    return value === 'client_secret_post' || value === 'client_secret_basic' || value === 'none'
}

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
    registration_endpoint?: string | null
    token_endpoint_auth_method?: OAuthTokenEndpointAuthMethod
    resource?: string | null
}

/// What flow we're driving — encoded into the OAuth state so the single
/// callback route can dispatch correctly.
export type OAuthFlow =
    /// Source admin/personal connect: triggers source creation or attaches
    /// per-user creds to an existing org source.
    | { type: 'connect_source'; sourceTypes: string[]; returnTo?: string }
    /// Admin attaches org-wide read/sync creds to a specific org source.
    | { type: 'org_source'; sourceId: string; returnTo?: string }
    /// User attaches per-user read creds to a specific org source.
    | { type: 'user_read'; sourceId: string; returnTo?: string }
    /// User attaches per-user read/write action creds to a specific org source.
    | {
          type: 'user_write'
          sourceId: string
          sourceType?: string
          returnTo?: string
          approvalId?: string
          approvalChatId?: string
      }

export interface ManifestOAuthState {
    user_id?: string
    metadata?: {
        flow: OAuthFlow
        provider: string
        requiredScopes: string[]
        // Granted-scope validation mode: writes require *exact* coverage of
        // requiredScopes; reads/identity don't.
        strictScopeCheck: boolean
        codeVerifier?: string
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
    clientSecret?: string
    tokenEndpointAuthMethod: OAuthTokenEndpointAuthMethod
    /// Optional per-deployment override for the manifest's auth_endpoint.
    /// Used when the auth URL has to embed deployment-specific data the
    /// connector can't know at compile time (e.g. Microsoft tenant id).
    authEndpoint?: string
    /// Same idea for the token endpoint.
    tokenEndpoint?: string
}

async function loadClientCreds(
    provider: string,
    manifestConfig?: OAuthManifestConfig,
): Promise<ClientCreds | null> {
    const row = await getConnectorConfig(provider)
    const storedConfig = (row?.config ?? {}) as Record<string, string>
    const storedMethod = storedConfig.oauth_token_endpoint_auth_method
    const tokenEndpointAuthMethod = isOAuthTokenEndpointAuthMethod(storedMethod)
        ? storedMethod
        : (manifestConfig?.token_endpoint_auth_method ?? 'client_secret_post')
    const clientId = storedConfig.oauth_client_id
    const clientSecret = storedConfig.oauth_client_secret

    if (clientId && isClientConfigComplete(storedConfig, tokenEndpointAuthMethod)) {
        return {
            clientId,
            clientSecret: clientSecret || undefined,
            tokenEndpointAuthMethod,
            authEndpoint: storedConfig.oauth_auth_endpoint || undefined,
            tokenEndpoint: storedConfig.oauth_token_endpoint || undefined,
        }
    }

    if (manifestConfig && isAutoManagedOAuthProvider(manifestConfig)) {
        return dynamicallyRegisterClient(provider, manifestConfig, storedConfig)
    }

    return null
}

async function dynamicallyRegisterClient(
    provider: string,
    config: OAuthManifestConfig,
    existingConfig: Record<string, string>,
): Promise<ClientCreds | null> {
    const redirectUri = callbackUrl()
    const scope = scopesForFlow(config, Object.keys(config.scopes), 'write').join(
        config.scope_separator,
    )
    let response: Response
    try {
        response = await fetch(config.registration_endpoint!, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                Accept: 'application/json',
            },
            body: JSON.stringify({
                client_name: 'Omni ClickUp MCP',
                redirect_uris: [redirectUri],
                grant_types: ['authorization_code'],
                response_types: ['code'],
                token_endpoint_auth_method: 'none',
                scope,
            }),
        })
    } catch {
        return null
    }
    const data = (await response.json().catch(() => ({}))) as { client_id?: string }
    if (!response.ok || !data.client_id) return null

    const stored = {
        ...existingConfig,
        oauth_client_id: data.client_id,
        oauth_token_endpoint_auth_method: 'none',
        oauth_dynamic_client_registration: 'true',
    }
    await upsertConnectorConfig(provider, stored, null)

    return {
        clientId: data.client_id,
        tokenEndpointAuthMethod: 'none',
        authEndpoint: existingConfig.oauth_auth_endpoint || undefined,
        tokenEndpoint: existingConfig.oauth_token_endpoint || undefined,
    }
}

export function tokenEndpointAuthMethodForConfig(
    config: Record<string, unknown> | undefined,
    manifestConfig?: OAuthManifestConfig,
): OAuthTokenEndpointAuthMethod {
    const storedMethod = config?.oauth_token_endpoint_auth_method
    if (isOAuthTokenEndpointAuthMethod(storedMethod)) return storedMethod
    return manifestConfig?.token_endpoint_auth_method ?? 'client_secret_post'
}

export function isClientConfigComplete(
    config: Record<string, unknown> | undefined,
    tokenEndpointAuthMethod: OAuthTokenEndpointAuthMethod,
): boolean {
    const clientId = config?.oauth_client_id
    const clientSecret = config?.oauth_client_secret
    if (typeof clientId !== 'string' || clientId.length === 0) return false
    if (tokenEndpointAuthMethod === 'none') return true
    return typeof clientSecret === 'string' && clientSecret.length > 0
}

export function isAutoManagedOAuthProvider(
    manifestConfig: OAuthManifestConfig | null | undefined,
): boolean {
    return Boolean(
        manifestConfig?.registration_endpoint &&
        manifestConfig.token_endpoint_auth_method === 'none',
    )
}

export async function isProviderConfigured(
    provider: string,
    manifestConfig?: OAuthManifestConfig,
): Promise<boolean> {
    if ((await loadClientCreds(provider, manifestConfig)) !== null) return true
    if (manifestConfig) return false
    const manifest = await getOAuthManifestForProvider(provider)
    return isAutoManagedOAuthProvider(manifest)
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
    flow: Extract<OAuthFlow, { type: 'connect_source' }>
    userId: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    const { flow, userId } = args
    const sourceType = flow.sourceTypes[0]

    const manifestConfig = await getOAuthManifestForSourceType(sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${sourceType}`)
    }

    const creds = await loadClientCreds(manifestConfig.provider, manifestConfig)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    // For `connect_source` we want read scopes (the source will sync); write
    // scopes are only granted by the explicit user_write flow.
    const mode: 'read' | 'write' = 'read'
    const sourceTypes = flow.type === 'connect_source' ? flow.sourceTypes : []
    const requiredScopes = scopesForFlow(manifestConfig, sourceTypes, mode)

    const pkce = pkceForConfig(manifestConfig)
    const { stateToken } = await OAuthStateManager.createState(
        manifestConfig.provider,
        callbackUrl(),
        userId,
        {
            flow,
            provider: manifestConfig.provider,
            requiredScopes,
            strictScopeCheck: false,
            codeVerifier: pkce?.verifier,
        },
    )

    return {
        url: buildAuthUrl(manifestConfig, creds, requiredScopes, stateToken, pkce?.challenge),
        requiredScopes,
    }
}

/// Variant for an admin org-source OAuth flow where the source has already
/// been created with provider-specific setup config.
export async function generateAuthUrlForOrgSource(args: {
    sourceId: string
    sourceType: string
    userId: string
    returnTo?: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    const manifestConfig = await getOAuthManifestForSourceType(args.sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${args.sourceType}`)
    }
    const creds = await loadClientCreds(manifestConfig.provider, manifestConfig)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    const requiredScopes = scopesForFlow(manifestConfig, [args.sourceType], 'read')
    const flow: OAuthFlow = {
        type: 'org_source',
        sourceId: args.sourceId,
        returnTo: args.returnTo,
    }

    const pkce = pkceForConfig(manifestConfig)
    const { stateToken } = await OAuthStateManager.createState(
        manifestConfig.provider,
        callbackUrl(),
        args.userId,
        {
            flow,
            provider: manifestConfig.provider,
            requiredScopes,
            strictScopeCheck: false,
            codeVerifier: pkce?.verifier,
        },
    )

    return {
        url: buildAuthUrl(manifestConfig, creds, requiredScopes, stateToken, pkce?.challenge),
        requiredScopes,
    }
}

async function generateAuthUrlForExistingSourceUserFlow(args: {
    sourceId: string
    sourceType: string
    userId: string
    returnTo?: string
    approvalId?: string
    approvalChatId?: string
    mode: 'read' | 'write'
}): Promise<{ url: string; requiredScopes: string[] }> {
    const manifestConfig = await getOAuthManifestForSourceType(args.sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${args.sourceType}`)
    }
    const creds = await loadClientCreds(manifestConfig.provider, manifestConfig)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    const readScopes = manifestConfig.scopes[args.sourceType]?.read ?? []
    const writeScopes = manifestConfig.scopes[args.sourceType]?.write ?? []
    const actionScopes =
        args.mode === 'write' ? [...new Set([...readScopes, ...writeScopes])] : readScopes
    if (actionScopes.length === 0) {
        throw new Error(`No ${args.mode} action scopes declared for source_type=${args.sourceType}`)
    }

    // Send identity + action scopes in the auth request. Strict-validate only
    // read/write action scopes for write flows — providers (Google) rewrite
    // identity scope aliases (`email` → `userinfo.email`) so equality on
    // identity scopes is fragile.
    const sentScopes = [...new Set([...manifestConfig.identity_scopes, ...actionScopes])]

    const flow: OAuthFlow = {
        type: args.mode === 'write' ? 'user_write' : 'user_read',
        sourceId: args.sourceId,
        ...(args.mode === 'write' ? { sourceType: args.sourceType } : {}),
        returnTo: args.returnTo,
        ...(args.mode === 'write' && args.approvalId ? { approvalId: args.approvalId } : {}),
        ...(args.mode === 'write' && args.approvalChatId
            ? { approvalChatId: args.approvalChatId }
            : {}),
    }

    const pkce = pkceForConfig(manifestConfig)
    const { stateToken } = await OAuthStateManager.createState(
        manifestConfig.provider,
        callbackUrl(),
        args.userId,
        {
            flow,
            provider: manifestConfig.provider,
            requiredScopes: actionScopes,
            strictScopeCheck: args.mode === 'write',
            codeVerifier: pkce?.verifier,
        },
    )

    return {
        url: buildAuthUrl(manifestConfig, creds, sentScopes, stateToken, pkce?.challenge),
        requiredScopes: actionScopes,
    }
}

/// Variant for the user-read flow where the caller already has the source's
/// source_type in hand.
export async function generateAuthUrlForUserRead(args: {
    sourceId: string
    sourceType: string
    userId: string
    returnTo?: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    return generateAuthUrlForExistingSourceUserFlow({ ...args, mode: 'read' })
}

/// Variant for the user-write flow where the caller already has the source's
/// source_type in hand.
export async function generateAuthUrlForUserWrite(args: {
    sourceId: string
    sourceType: string
    userId: string
    returnTo?: string
    approvalId?: string
    approvalChatId?: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    return generateAuthUrlForExistingSourceUserFlow({ ...args, mode: 'write' })
}

function pkceForConfig(
    config: OAuthManifestConfig,
): { verifier: string; challenge: string } | null {
    if (config.token_endpoint_auth_method !== 'none') return null
    const verifier = randomBytes(32).toString('base64url')
    const challenge = createHash('sha256').update(verifier).digest('base64url')
    return { verifier, challenge }
}

function buildAuthUrl(
    config: OAuthManifestConfig,
    creds: ClientCreds,
    scopes: string[],
    stateToken: string,
    codeChallenge?: string,
): string {
    const params = new URLSearchParams({
        client_id: creds.clientId,
        redirect_uri: callbackUrl(),
        response_type: 'code',
        scope: scopes.join(config.scope_separator),
        state: stateToken,
        ...config.extra_auth_params,
    })
    if (codeChallenge) {
        params.set('code_challenge', codeChallenge)
        params.set('code_challenge_method', 'S256')
    }
    const authEndpoint = creds.authEndpoint ?? config.auth_endpoint
    return `${authEndpoint}?${params.toString()}`
}

export interface ExchangeResult {
    tokens: OAuthTokens
    state: ManifestOAuthState
    config: OAuthManifestConfig
    principalEmail: string
    clientCreds: ClientCreds
}

export interface ExchangeCodeOptions {
    /// Provider-scoped identity overrides for providers whose OAuth token cannot
    /// be used with their REST userinfo endpoint. Values must come from an
    /// already-authenticated Omni user, never from request input.
    principalEmailOverrides?: Record<string, string>
}

/// Exchange an authorization code for tokens, validate state, fetch
/// principal email unless a trusted provider-specific override is supplied.
export async function exchangeCodeAndIdentify(
    code: string,
    stateToken: string,
    options: ExchangeCodeOptions = {},
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

    const creds = await loadClientCreds(provider, config)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${provider}`)
    }

    const tokenParams = new URLSearchParams({
        client_id: creds.clientId,
        code,
        grant_type: 'authorization_code',
        redirect_uri: callbackUrl(),
    })
    const tokenHeaders: Record<string, string> = {
        'Content-Type': 'application/x-www-form-urlencoded',
        Accept: 'application/json',
    }
    if (creds.tokenEndpointAuthMethod === 'client_secret_post' && creds.clientSecret) {
        tokenParams.set('client_secret', creds.clientSecret)
    } else if (creds.tokenEndpointAuthMethod === 'client_secret_basic' && creds.clientSecret) {
        tokenParams.delete('client_id')
        tokenHeaders.Authorization = `Basic ${Buffer.from(
            `${creds.clientId}:${creds.clientSecret}`,
        ).toString('base64')}`
    }
    if (state.metadata.codeVerifier) {
        tokenParams.set('code_verifier', state.metadata.codeVerifier)
    }
    if (config.resource) {
        tokenParams.set('resource', config.resource)
    }

    const tokenEndpoint = creds.tokenEndpoint ?? config.token_endpoint
    logger.info('Starting connector OAuth token exchange', {
        provider: config.provider,
        flow: state.metadata.flow.type,
        tokenEndpoint,
        tokenEndpointAuthMethod: creds.tokenEndpointAuthMethod,
        clientIdPrefix: creds.clientId.slice(0, 12),
        redirectUri: callbackUrl(),
        hasPkceVerifier: Boolean(state.metadata.codeVerifier),
        resource: config.resource ?? null,
        requestedScopes: state.metadata.requiredScopes,
    })
    const tokenResp = await fetch(tokenEndpoint, {
        method: 'POST',
        headers: tokenHeaders,
        body: tokenParams.toString(),
    })
    const tokenData = (await tokenResp.json().catch(() => ({}))) as OAuthTokens | OAuthError
    if (!tokenResp.ok) {
        const err = tokenData as OAuthError
        logger.warn('Connector OAuth token exchange failed', {
            provider: config.provider,
            flow: state.metadata.flow.type,
            status: tokenResp.status,
            tokenEndpoint,
            tokenEndpointAuthMethod: creds.tokenEndpointAuthMethod,
            clientIdPrefix: creds.clientId.slice(0, 12),
            resource: config.resource ?? null,
            error: err.error,
            errorDescription: err.error_description,
        })
        throw new Error(`OAuth token exchange failed: ${err.error} - ${err.error_description}`)
    }
    const tokens = tokenData as OAuthTokens
    logger.info('Connector OAuth token exchange succeeded', {
        provider: config.provider,
        flow: state.metadata.flow.type,
        tokenType: tokens.token_type,
        expiresIn: tokens.expires_in,
        grantedScope: tokens.scope ?? null,
    })

    const principalEmailOverride = options.principalEmailOverrides?.[config.provider]
    if (principalEmailOverride) {
        logger.info('Using authenticated Omni user as connector OAuth principal', {
            provider: config.provider,
            flow: state.metadata.flow.type,
        })
        return { tokens, state, config, principalEmail: principalEmailOverride, clientCreds: creds }
    }

    const userinfoResp = await fetch(config.userinfo_endpoint, {
        headers: {
            Authorization: `Bearer ${tokens.access_token}`,
            Accept: 'application/json',
        },
    })
    if (!userinfoResp.ok) {
        const body = await userinfoResp.text().catch(() => '')
        logger.warn('Connector OAuth userinfo fetch failed', {
            provider: config.provider,
            flow: state.metadata.flow.type,
            status: userinfoResp.status,
            userinfoEndpoint: config.userinfo_endpoint,
            body: body.slice(0, 500),
        })
        throw new Error(`Failed to fetch userinfo: ${userinfoResp.status}`)
    }
    const profile = (await userinfoResp.json()) as unknown
    const email = extractEmailFromUserinfo(profile, config.userinfo_email_field)
    if (!email) {
        logger.warn('Connector OAuth userinfo response missing email field', {
            provider: config.provider,
            flow: state.metadata.flow.type,
            userinfoEmailField: config.userinfo_email_field,
            profileKeys: isUserinfoObject(profile) ? Object.keys(profile) : null,
        })
        throw new Error(`userinfo response missing field "${config.userinfo_email_field}"`)
    }

    return { tokens, state, config, principalEmail: email, clientCreds: creds }
}

function extractEmailFromUserinfo(profile: unknown, emailField: string): string | null {
    if (Array.isArray(profile)) {
        const entries = profile.filter(isUserinfoObject)
        return (
            getStringField(
                entries.find((entry) => entry.primary === true && entry.verified === true),
                emailField,
            ) ??
            getStringField(
                entries.find((entry) => entry.verified === true),
                emailField,
            ) ??
            getStringField(
                entries.find((entry) => typeof entry[emailField] === 'string'),
                emailField,
            )
        )
    }

    return getStringField(isUserinfoObject(profile) ? profile : null, emailField)
}

function isUserinfoObject(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null
}

function getStringField(
    value: Record<string, unknown> | null | undefined,
    field: string,
): string | null {
    const fieldValue = field.split('.').reduce<unknown>((current, part) => {
        if (!isUserinfoObject(current)) return undefined
        return current[part]
    }, value)
    return typeof fieldValue === 'string' && fieldValue ? fieldValue : null
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
