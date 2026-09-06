import { createHash, randomBytes } from 'crypto'
import { sql } from 'drizzle-orm'
import { app, getConfig } from '../config'
import { db } from '../db'
import {
    deleteConnectorConfig,
    getConnectorConfig,
    upsertConnectorConfig,
} from '../db/connector-configs'
import { getSourceById } from '../db/sources'
import type { Source } from '../db/schema'
import { createLogger } from '../logger'
import {
    discoverRemoteMcpOAuthConfig,
    fetchWithPinnedRemoteMcpDns,
    readRemoteMcpLimitedJson,
    validateRemoteMcpUrl,
    validateRemoteMcpUrlForCredentialUse,
} from '../mcp/client'
import type { RemoteMcpIpPolicy } from '../mcp/client'
import { OAuthStateManager } from './state'
import type { OAuthError, OAuthTokens } from './types'
import { IntegrationType } from '$lib/types'

export type OAuthTokenEndpointAuthMethod = 'client_secret_post' | 'client_secret_basic' | 'none'

const logger = createLogger('connector-oauth')
const OAUTH_RESPONSE_MAX_BYTES = 1024 * 1024

/**
 * Lifetime assumed when a provider omits `expires_in` from its token
 * response (e.g. Salesforce). Mirrors the connector-manager's refresh
 * default; erring short only causes an earlier refresh-token exchange.
 */
export const DEFAULT_OAUTH_EXPIRES_IN_SECONDS = 3600

/// Source config key under which a connector-validated OAuth source binding
/// is stored verbatim. Nothing else in source config is written from the
/// validation response.
export const SOURCE_BINDING_CONFIG_KEY = 'source_binding'

/** Typed provider-defined identity binding returned by OAuth validation. */
export type OAuthSourceBinding = Record<string, string>

/**
 * Parse the `source_binding` field of a connector's OAuth validation
 * response. Bindings are provider-defined string→string maps (e.g. a
 * Salesforce `organization_id`); they are stored verbatim under the reserved
 * `source_binding` source config key. Returns null when absent; throws on
 * malformed bindings so a misbehaving connector cannot smuggle arbitrary
 * config through.
 */
export function parseOAuthSourceBinding(body: unknown): OAuthSourceBinding | null {
    if (typeof body !== 'object' || body === null || Array.isArray(body)) {
        throw new Error('OAuth validation returned an invalid response')
    }
    const binding = (body as { source_binding?: unknown }).source_binding
    if (binding === undefined || binding === null) {
        return null
    }
    if (typeof binding !== 'object' || Array.isArray(binding)) {
        throw new Error('OAuth validation returned an invalid source binding')
    }
    const result: OAuthSourceBinding = {}
    for (const [key, value] of Object.entries(binding as Record<string, unknown>)) {
        if (!key) {
            throw new Error('OAuth validation returned an invalid binding field name')
        }
        if (typeof value !== 'string') {
            throw new Error(`OAuth validation returned an invalid binding field: ${key}`)
        }
        result[key] = value
    }
    return result
}

/**
 * Ask the connector (via connector-manager) to validate an OAuth credential
 * and return the source binding it discovered, if any. `flow` is passed
 * through typed so connectors can tell setup-time org credentials apart from
 * user-attached ones.
 */
export async function requestOAuthCredentialValidation(args: {
    sourceId: string
    provider: string
    credentials: Record<string, unknown>
    flow: 'org_source' | 'connect_source' | 'user_read' | 'user_write'
    metadata?: Record<string, unknown>
}): Promise<OAuthSourceBinding | null> {
    const response = await fetch(`${getConfig().services.connectorManagerUrl}/oauth/validate`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
            source_id: args.sourceId,
            provider: args.provider,
            credentials: args.credentials,
            flow: args.flow,
            metadata: args.metadata ?? {},
        }),
    })
    const body = (await response.json().catch(() => null)) as unknown
    if (!response.ok) {
        const message =
            typeof body === 'object' && body !== null
                ? 'message' in body
                    ? (body as { message?: unknown }).message
                    : 'error' in body
                      ? (body as { error?: unknown }).error
                      : undefined
                : undefined
        throw new Error(
            typeof message === 'string' && message ? message : 'OAuth credential rejected',
        )
    }
    return parseOAuthSourceBinding(body)
}

/**
 * Credential expiry derived from a token response. When the provider omits
 * `expires_in`, fall back to the shared default lifetime — but only when a
 * refresh token exists (the manager only refreshes rows whose expires_at has
 * arrived). Without a refresh token there is nothing to refresh, so the
 * credential is stored with no expiry instead of a fabricated one.
 */
export function oauthCredentialExpiry(
    tokens: { expires_in?: unknown },
    refreshToken?: unknown,
): Date | null {
    if (typeof tokens.expires_in === 'number' && tokens.expires_in > 0) {
        return new Date(Date.now() + tokens.expires_in * 1000)
    }
    if (typeof refreshToken === 'string' && refreshToken.length > 0) {
        return new Date(Date.now() + DEFAULT_OAUTH_EXPIRES_IN_SECONDS * 1000)
    }
    return null
}

function isOAuthTokenEndpointAuthMethod(value: unknown): value is OAuthTokenEndpointAuthMethod {
    return value === 'client_secret_post' || value === 'client_secret_basic' || value === 'none'
}

function isOAuthError(value: unknown): value is OAuthError {
    return (
        typeof value === 'object' &&
        value !== null &&
        !Array.isArray(value) &&
        typeof (value as { error?: unknown }).error === 'string' &&
        (value as { error: string }).error.length > 0
    )
}

function isOAuthTokens(value: unknown): value is OAuthTokens {
    return (
        typeof value === 'object' &&
        value !== null &&
        !Array.isArray(value) &&
        typeof (value as { access_token?: unknown }).access_token === 'string' &&
        (value as { access_token: string }).access_token.length > 0
    )
}

interface SlackOAuthTokenResponse {
    token_type?: string
    scope?: string
    authed_user?: {
        access_token?: string
        token_type?: string
        scope?: string
    }
}

export function normalizeOAuthTokens(provider: string, tokenData: unknown): OAuthTokens {
    if (provider === 'slack') {
        const slackData =
            typeof tokenData === 'object' && tokenData !== null && !Array.isArray(tokenData)
                ? (tokenData as SlackOAuthTokenResponse)
                : null
        const user = slackData?.authed_user
        if (!user?.access_token) {
            throw new Error('Slack OAuth response did not contain a delegated user access token')
        }
        return {
            access_token: user.access_token,
            token_type: user.token_type ?? slackData.token_type ?? 'Bearer',
            scope: user.scope ?? slackData.scope,
        }
    }
    if (!isOAuthTokens(tokenData)) {
        throw new Error('OAuth token exchange returned an invalid token response')
    }
    return tokenData
}

/// Mirrors `shared::models::OAuthManifestConfig` (Rust). Pure data: a connector
/// declares this in its manifest and the web app's generic OAuth2 client uses
/// it to drive the standard authorization-code flow.
export interface OAuthManifestConfig {
    provider: string
    auth_endpoint: string
    token_endpoint: string
    userinfo_endpoint?: string | null
    userinfo_email_field: string
    identity_scopes: string[]
    scopes: Record<string, { read: string[]; write: string[] }>
    extra_auth_params: Record<string, string>
    scope_separator: string
    scope_parameter?: string
    user_auth_for_writes_only?: boolean
    enrich_endpoint?: string | null
    registration_endpoint?: string | null
    registration_requires_initial_access_token?: boolean
    token_response_fields?: string[]
    token_endpoint_auth_method?: OAuthTokenEndpointAuthMethod
    resource?: string | null
    credential_provider?: string | null
    protected_resource_metadata_url?: string | null
    authorization_server_metadata_url?: string | null
    /// Operator-configured private route (Windshift only): when present, the
    /// transport endpoints (token/registration/userinfo) are derived from
    /// this URL and may resolve to RFC1918 addresses. Absent for every other
    /// provider and when no internal route is configured.
    internal_base_url?: string | null
    /// Source-scoped client configuration key. This is intentionally not a
    /// provider identity; credentials stored in service_credentials continue
    /// to use the logical provider name.
    client_config_provider?: string | null
    /// Optional source config key containing an OAuth issuer URL. When set,
    /// the web client resolves the issuer through standard OIDC discovery.
    issuer_source_config_key?: string | null
    /// Optional template for source-scoped OAuth client configuration. The
    /// `{source_id}` placeholder is replaced for a persisted source.
    client_config_provider_template?: string | null
    /// Whether authorization requests must use PKCE.
    pkce_required?: boolean
    /// Optional OAuth Dynamic Client Registration grant types.
    grant_types?: string[] | null
    /// Whether manifest OAuth endpoints require SSRF-safe URL validation.
    validate_endpoint_urls?: boolean
    /// Whether this connector supports OAuth credentials for org sources.
    supports_org_oauth?: boolean
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

/** Return the public service URL represented by an OAuth authorization endpoint. */
export function oauthServiceBaseUrl(authEndpoint: string): string {
    const authorizationPath = '/oauth/authorize'
    try {
        const url = new URL(authEndpoint)
        if (url.pathname.endsWith(authorizationPath)) {
            url.pathname = url.pathname.slice(0, -authorizationPath.length) || '/'
        }
        url.search = ''
        url.hash = ''
        return url.toString().replace(/\/$/, '')
    } catch {
        return authEndpoint.replace(/\/oauth\/authorize\/?$/, '').replace(/\/$/, '')
    }
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
    const body: unknown = await resp.json().catch(() => null)
    if (!Array.isArray(body)) return null
    const entry = body.find(
        (value): value is Record<string, unknown> =>
            isRecord(value) && value.source_type === sourceType,
    )
    return oauthManifestFromResponse(entry)
}

function normalizeOAuthUrl(value: unknown): URL | null {
    if (typeof value !== 'string' || !value.trim()) return null
    try {
        const url = new URL(value.trim())
        if (
            url.protocol !== 'https:' ||
            url.username ||
            url.password ||
            url.port ||
            url.search ||
            url.hash
        ) {
            return null
        }
        url.pathname = url.pathname.replace(/\/+$/, '') || '/'
        return url
    } catch {
        return null
    }
}

async function validateOAuthEndpoint(value: unknown): Promise<string | null> {
    const url = normalizeOAuthUrl(value)
    if (!url) return null
    try {
        const validated = await validateRemoteMcpUrlForCredentialUse(url.toString())
        return validateRemoteMcpUrl(validated).toString().replace(/\/$/, '')
    } catch {
        return null
    }
}

async function discoverOAuthManifestFromIssuer(
    manifest: OAuthManifestConfig,
    issuerValue: unknown,
): Promise<OAuthManifestConfig | null> {
    const issuer = normalizeOAuthUrl(issuerValue)
    if (!issuer) return null

    const issuerPath = issuer.pathname.replace(/\/$/, '')
    const metadataUrl = new URL(`${issuerPath}/.well-known/openid-configuration`, issuer.origin)
    let metadata: Record<string, unknown>
    try {
        const validatedMetadataUrl = await validateOAuthEndpoint(metadataUrl.toString())
        if (!validatedMetadataUrl) return null
        const response = await fetchWithPinnedRemoteMcpDns(
            new URL(validatedMetadataUrl),
            {
                headers: { Accept: 'application/json' },
                signal: AbortSignal.timeout(10_000),
            },
            {},
        )
        if (!response.ok) return null
        const body = JSON.parse(await readLimitedResponseText(response)) as unknown
        if (typeof body !== 'object' || body === null || Array.isArray(body)) return null
        metadata = body as Record<string, unknown>
    } catch {
        return null
    }

    const metadataIssuer = normalizeOAuthUrl(metadata.issuer)
    if (!metadataIssuer || metadataIssuer.toString() !== issuer.toString()) return null

    const fallbackEndpoint = (configured: string | null | undefined): string | null => {
        const configuredUrl = normalizeOAuthUrl(configured)
        return configuredUrl
            ? new URL(configuredUrl.pathname, issuer.origin).toString().replace(/\/$/, '')
            : null
    }
    const authEndpoint = await validateOAuthEndpoint(
        metadata.authorization_endpoint ?? fallbackEndpoint(manifest.auth_endpoint),
    )
    const tokenEndpoint = await validateOAuthEndpoint(
        metadata.token_endpoint ?? fallbackEndpoint(manifest.token_endpoint),
    )
    if (!authEndpoint || !tokenEndpoint) return null

    const userinfoRequired = manifest.userinfo_endpoint != null
    const registrationRequired = manifest.registration_endpoint != null
    const userinfoEndpoint = await validateOAuthEndpoint(
        metadata.userinfo_endpoint ?? fallbackEndpoint(manifest.userinfo_endpoint),
    )
    const registrationEndpoint = await validateOAuthEndpoint(
        metadata.registration_endpoint ?? fallbackEndpoint(manifest.registration_endpoint),
    )
    if (
        (userinfoRequired && !userinfoEndpoint) ||
        (registrationRequired && !registrationEndpoint)
    ) {
        return null
    }

    return {
        ...manifest,
        auth_endpoint: authEndpoint,
        token_endpoint: tokenEndpoint,
        userinfo_endpoint: userinfoEndpoint ?? undefined,
        registration_endpoint: registrationEndpoint ?? undefined,
    }
}

export function clientConfigProviderForSource(
    manifest: OAuthManifestConfig,
    sourceId: string,
): string {
    return (
        manifest.client_config_provider_template?.replaceAll('{source_id}', sourceId) ??
        manifest.client_config_provider ??
        manifest.provider
    )
}

function resolveSourceClientConfigProvider(
    manifest: OAuthManifestConfig,
    sourceId?: string,
): OAuthManifestConfig {
    if (!sourceId) return manifest
    return {
        ...manifest,
        client_config_provider: clientConfigProviderForSource(manifest, sourceId),
    }
}

type OAuthSource = Pick<Source, 'sourceType' | 'integrationType' | 'config'> &
    Partial<Pick<Source, 'id'>>

export async function getOAuthConfigForSource(
    source: OAuthSource,
): Promise<OAuthManifestConfig | null> {
    if (source.integrationType !== IntegrationType.REMOTE_MCP) {
        const manifest = await getOAuthManifestForSourceType(source.sourceType)
        if (!manifest) return null

        let resolved: OAuthManifestConfig | null = manifest
        const sourceConfig = (source.config ?? {}) as Record<string, unknown>
        const issuerKey = manifest.issuer_source_config_key
        if (issuerKey && Object.prototype.hasOwnProperty.call(sourceConfig, issuerKey)) {
            resolved =
                (await discoverOAuthManifestFromIssuer(manifest, sourceConfig[issuerKey])) ?? null
            if (!resolved) return null
        }
        return resolveSourceClientConfigProvider(resolved, source.id)
    }

    const provider = `remote_mcp:${source.sourceType}`
    const manifestConfig = await getOAuthManifestForSourceType(source.sourceType)
    if (manifestConfig?.provider === provider) return manifestConfig

    return (await discoverRemoteMcpOAuthConfig({
        endpointUrl: String((source.config as Record<string, unknown>)?.endpoint_url ?? ''),
        sourceType: source.sourceType,
    })) as OAuthManifestConfig | null
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
    const clientConfigProvider = manifestConfig?.client_config_provider || provider
    // An explicit source-scoped config key must not fall back to the
    // provider-global client, which could use another source's credentials.
    const allowProviderFallback = clientConfigProvider === provider
    const row =
        (await getConnectorConfig(clientConfigProvider)) ??
        (allowProviderFallback ? await getConnectorConfig(provider) : null)
    const storedConfig = (row?.config ?? {}) as Record<string, string>
    const storedMethod = storedConfig.oauth_token_endpoint_auth_method
    const tokenEndpointAuthMethod = isOAuthTokenEndpointAuthMethod(storedMethod)
        ? storedMethod
        : (manifestConfig?.token_endpoint_auth_method ?? 'client_secret_post')
    const clientId = storedConfig.oauth_client_id
    const clientSecret = storedConfig.oauth_client_secret
    const clientSecretExpiresAt = Number(storedConfig.oauth_client_secret_expires_at ?? 0)
    const clientSecretExpired =
        clientSecretExpiresAt > 0 && clientSecretExpiresAt <= Math.floor(Date.now() / 1000)

    const dynamicRegistrationIsCurrent =
        !manifestConfig ||
        !isAutoManagedOAuthProvider(manifestConfig) ||
        storedConfig.oauth_dynamic_client_registration !== 'true' ||
        (storedConfig.oauth_registration_endpoint === manifestConfig.registration_endpoint &&
            storedConfig.oauth_redirect_uri === callbackUrl())

    if (
        clientId &&
        !clientSecretExpired &&
        isClientConfigComplete(storedConfig, tokenEndpointAuthMethod) &&
        dynamicRegistrationIsCurrent
    ) {
        return {
            clientId,
            clientSecret: clientSecret || undefined,
            tokenEndpointAuthMethod,
            authEndpoint: storedConfig.oauth_auth_endpoint || undefined,
            tokenEndpoint: storedConfig.oauth_token_endpoint || undefined,
        }
    }

    if (manifestConfig && isAutoManagedOAuthProvider(manifestConfig)) {
        return dynamicallyRegisterClient(
            provider,
            manifestConfig,
            storedConfig,
            clientConfigProvider,
        )
    }

    return null
}

/// Providers whose OAuth endpoints are admin-configured URLs (remote MCP
/// servers, Windshift). Server-side fetches to these endpoints must go
/// through SSRF validation and pinned-DNS resolution.
function isAdminConfiguredEndpointProvider(provider: string): boolean {
    return provider.startsWith('remote_mcp:') || provider === 'windshift'
}

function requiresOAuthEndpointValidation(provider: string, config?: OAuthManifestConfig): boolean {
    return isAdminConfiguredEndpointProvider(provider) || config?.validate_endpoint_urls === true
}

/// The exact origin (scheme://host:port) of the operator-configured Windshift
/// internal route, or null when unset/invalid. The connector advertises it in
/// the manifest only when WINDSHIFT_INTERNAL_BASE_URL is set, so a manifest
/// without the marker (public-only deployment) stays strictly public.
/// Endpoints on this origin may resolve to RFC1918 private addresses — the
/// internal URL's purpose is private networking; every other endpoint must
/// stay publicly routable.
export function windshiftInternalOrigin(config: OAuthManifestConfig): string | null {
    if (config.provider !== 'windshift') return null
    const internalBaseUrl = config.internal_base_url
    if (typeof internalBaseUrl !== 'string' || !internalBaseUrl) return null
    try {
        return new URL(internalBaseUrl).origin
    } catch {
        return null
    }
}

function ssrfPolicyForEndpoint(endpoint: string, internalOrigin: string | null): RemoteMcpIpPolicy {
    if (!internalOrigin) return {}
    try {
        if (new URL(endpoint).origin === internalOrigin) return { allowPrivate: true }
    } catch {
        // Not a URL (e.g. a resource identifier): keep strict.
    }
    return {}
}

async function remoteMcpCredentialFetch(
    provider: string,
    endpoint: string,
    init: RequestInit,
    internalOrigin: string | null,
    validateEndpoint = false,
): Promise<Response> {
    if (!isAdminConfiguredEndpointProvider(provider) && !validateEndpoint) {
        return fetch(endpoint, {
            ...init,
            signal: init.signal ?? AbortSignal.timeout(20_000),
        })
    }
    const policy = ssrfPolicyForEndpoint(endpoint, internalOrigin)
    const validated = await validateRemoteMcpUrlForCredentialUse(endpoint, policy)
    return fetchWithPinnedRemoteMcpDns(validateRemoteMcpUrl(validated), init, policy)
}

async function readLimitedResponseText(response: Response): Promise<string> {
    const reader = response.body?.getReader()
    if (!reader) return ''
    const chunks: Uint8Array[] = []
    let total = 0
    while (true) {
        const { done, value } = await reader.read()
        if (done) break
        total += value.byteLength
        if (total > OAUTH_RESPONSE_MAX_BYTES) throw new Error('OAuth response too large')
        chunks.push(value)
    }
    return new TextDecoder().decode(Buffer.concat(chunks))
}

async function readCredentialJson(provider: string, response: Response): Promise<unknown> {
    if (isAdminConfiguredEndpointProvider(provider)) return readRemoteMcpLimitedJson(response)
    const text = await readLimitedResponseText(response)
    return text.trim() ? JSON.parse(text) : {}
}

async function readCredentialText(_provider: string, response: Response): Promise<string> {
    return readLimitedResponseText(response)
}

async function validateRemoteMcpOAuthConfigUrls(config: OAuthManifestConfig): Promise<void> {
    if (!requiresOAuthEndpointValidation(config.provider, config)) return
    const internalOrigin = windshiftInternalOrigin(config)
    const endpoints = [config.auth_endpoint, config.token_endpoint]
    if (config.userinfo_endpoint) endpoints.push(config.userinfo_endpoint)
    if (config.registration_endpoint) endpoints.push(config.registration_endpoint)
    if (config.resource) endpoints.push(config.resource)
    if (config.protected_resource_metadata_url) {
        endpoints.push(config.protected_resource_metadata_url)
    }
    if (config.authorization_server_metadata_url) {
        endpoints.push(config.authorization_server_metadata_url)
    }
    for (const endpoint of endpoints) {
        await validateRemoteMcpUrlForCredentialUse(
            endpoint,
            ssrfPolicyForEndpoint(endpoint, internalOrigin),
        )
    }
}

async function dynamicallyRegisterClient(
    provider: string,
    config: OAuthManifestConfig,
    existingConfig: Record<string, string>,
    clientConfigProvider = provider,
): Promise<ClientCreds | null> {
    // Registration is a read/claim/write sequence. Serialize it across web
    // instances with a transaction-scoped PostgreSQL advisory lock so two
    // workers cannot spend the same initial-access token creating duplicate
    // clients.
    return db.transaction(async (tx) => {
        await tx.execute(
            sql`select pg_advisory_xact_lock(hashtext(${`oauth-dcr:${clientConfigProvider}`}))`,
        )
        const latest = await getConnectorConfig(clientConfigProvider)
        const latestConfig = (latest?.config ?? existingConfig) as Record<string, string>
        return dynamicallyRegisterClientUnlocked(
            provider,
            config,
            latestConfig,
            clientConfigProvider,
        )
    })
}

async function dynamicallyRegisterClientUnlocked(
    provider: string,
    config: OAuthManifestConfig,
    existingConfig: Record<string, string>,
    clientConfigProvider = provider,
): Promise<ClientCreds | null> {
    const redirectUri = callbackUrl()
    const scope = scopesForFlow(config, Object.keys(config.scopes), 'write').join(
        config.scope_separator,
    )
    const tokenEndpointAuthMethod = config.token_endpoint_auth_method ?? 'none'
    const initialAccessToken = existingConfig.oauth_registration_initial_access_token
    const lastAttempt = Number(existingConfig.oauth_registration_attempted_at ?? 0)
    if (Number.isFinite(lastAttempt) && Date.now() - lastAttempt < 60_000) return null
    // Some providers require an authorized initial access token and return
    // confidential-client credentials. Never attempt an unauthenticated or
    // secretless registration when the manifest requires one.
    if (config.registration_requires_initial_access_token && !initialAccessToken) {
        return null
    }

    // Do not accumulate abandoned clients when a registration becomes stale
    // (for example after the callback URL or registration endpoint changes).
    // If the provider cannot revoke the old client, fail closed instead of
    // creating another one with the same administrator token.
    if (existingConfig.oauth_dynamic_client_registration === 'true') {
        const hadRegistration =
            typeof existingConfig.oauth_registration_client_uri === 'string' &&
            typeof existingConfig.oauth_registration_access_token === 'string'
        if (
            !hadRegistration ||
            !(await revokeDynamicallyRegisteredClient(clientConfigProvider, existingConfig))
        ) {
            logger.warn(`Cannot replace stale OAuth DCR client for ${clientConfigProvider}`)
            return null
        }
    }

    let response: Response
    try {
        await validateRemoteMcpOAuthConfigUrls(config)
        await upsertConnectorConfig(
            clientConfigProvider,
            { ...existingConfig, oauth_registration_attempted_at: String(Date.now()) },
            null,
        )
        const headers: Record<string, string> = {
            'Content-Type': 'application/json',
            Accept: 'application/json',
        }
        if (initialAccessToken) headers.Authorization = `Bearer ${initialAccessToken}`
        response = await remoteMcpCredentialFetch(
            provider,
            config.registration_endpoint!,
            {
                method: 'POST',
                headers,
                body: JSON.stringify(
                    dynamicRegistrationPayload(
                        provider,
                        redirectUri,
                        scope,
                        tokenEndpointAuthMethod,
                        config.grant_types ?? undefined,
                    ),
                ),
            },
            windshiftInternalOrigin(config),
            config.validate_endpoint_urls === true,
        )
    } catch {
        return null
    }
    const data = (await readCredentialJson(provider, response).catch(() => ({}))) as {
        client_id?: string
        client_secret?: string
        client_secret_expires_at?: number
        registration_client_uri?: string
        registration_access_token?: string
        token_endpoint_auth_method?: OAuthTokenEndpointAuthMethod
    }
    if (!response.ok || !data.client_id) return null
    const registeredAuthMethod = isOAuthTokenEndpointAuthMethod(data.token_endpoint_auth_method)
        ? data.token_endpoint_auth_method
        : tokenEndpointAuthMethod
    if (registeredAuthMethod !== 'none' && !data.client_secret) return null

    const configWithoutRegistrationMetadata = { ...existingConfig }
    delete configWithoutRegistrationMetadata.oauth_registration_initial_access_token
    delete configWithoutRegistrationMetadata.oauth_registration_client_uri
    delete configWithoutRegistrationMetadata.oauth_registration_access_token
    const stored = {
        ...configWithoutRegistrationMetadata,
        oauth_client_id: data.client_id,
        ...(data.client_secret ? { oauth_client_secret: data.client_secret } : {}),
        ...(data.client_secret_expires_at
            ? { oauth_client_secret_expires_at: String(data.client_secret_expires_at) }
            : {}),
        ...(data.registration_client_uri
            ? { oauth_registration_client_uri: data.registration_client_uri }
            : {}),
        ...(data.registration_access_token
            ? { oauth_registration_access_token: data.registration_access_token }
            : {}),
        oauth_token_endpoint_auth_method: registeredAuthMethod,
        oauth_dynamic_client_registration: 'true',
        oauth_registration_endpoint: config.registration_endpoint!,
        oauth_redirect_uri: redirectUri,
    }
    await upsertConnectorConfig(clientConfigProvider, stored, null)

    return {
        clientId: data.client_id,
        clientSecret: data.client_secret || undefined,
        tokenEndpointAuthMethod: registeredAuthMethod,
        authEndpoint: existingConfig.oauth_auth_endpoint || undefined,
        tokenEndpoint: existingConfig.oauth_token_endpoint || undefined,
    }
}

/**
 * Revoke an RFC 7592 dynamically registered client when the provider exposes
 * registration-management credentials.
 */
export async function revokeDynamicallyRegisteredClient(
    provider: string,
    config: Record<string, unknown>,
): Promise<boolean> {
    const registrationUri = config.oauth_registration_client_uri
    const registrationAccessToken = config.oauth_registration_access_token
    if (typeof registrationUri !== 'string' || typeof registrationAccessToken !== 'string') {
        return false
    }

    let clientUrl: URL
    let registrationUrl: URL
    try {
        clientUrl = new URL(registrationUri)
        registrationUrl = new URL(String(config.oauth_registration_endpoint ?? ''))
    } catch {
        logger.warn(`Skipping invalid OAuth DCR management URL for ${provider}`)
        return false
    }
    if (
        clientUrl.protocol !== 'https:' ||
        clientUrl.username ||
        clientUrl.password ||
        clientUrl.search ||
        clientUrl.hash ||
        clientUrl.origin !== registrationUrl.origin
    ) {
        logger.warn(`Skipping untrusted OAuth DCR management URL for ${provider}`)
        return false
    }

    try {
        const validated = await validateRemoteMcpUrlForCredentialUse(registrationUri)
        const response = await fetchWithPinnedRemoteMcpDns(new URL(validated), {
            method: 'DELETE',
            headers: { Authorization: `Bearer ${registrationAccessToken}` },
            signal: AbortSignal.timeout(20_000),
        })
        if (!response.ok && response.status !== 404) {
            logger.warn(
                `OAuth DCR client deletion returned HTTP ${response.status} for ${provider}`,
            )
            return false
        }
        return true
    } catch (error) {
        logger.warn(`OAuth DCR client deletion failed for ${provider}`, error)
        return false
    }
}

export async function removeDynamicallyRegisteredClient(provider: string): Promise<void> {
    const existing = await getConnectorConfig(provider)
    if (existing) {
        const config = existing.config as Record<string, unknown>
        const hasDynamicClient = config.oauth_dynamic_client_registration === 'true'
        const hasRegistrationMetadata =
            typeof config.oauth_registration_client_uri === 'string' &&
            typeof config.oauth_registration_access_token === 'string'
        if (
            (hasDynamicClient || hasRegistrationMetadata) &&
            !(await revokeDynamicallyRegisteredClient(provider, config))
        ) {
            throw new Error(`Could not revoke OAuth DCR client for ${provider}`)
        }
    }
    await deleteConnectorConfig(provider)
}

export function dynamicRegistrationPayload(
    provider: string,
    redirectUri: string,
    scope: string,
    tokenEndpointAuthMethod: OAuthTokenEndpointAuthMethod = 'none',
    grantTypes?: string[],
) {
    const providerName =
        provider === 'clickup'
            ? 'ClickUp'
            : provider
                  .split(/[-_\s]+/)
                  .filter(Boolean)
                  .map((part) => part[0].toUpperCase() + part.slice(1))
                  .join(' ')
    return {
        client_name: `Omni ${providerName} MCP`,
        redirect_uris: [redirectUri],
        grant_types: grantTypes ?? ['authorization_code'],
        response_types: ['code'],
        token_endpoint_auth_method: tokenEndpointAuthMethod,
        scope,
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
        (manifestConfig.token_endpoint_auth_method === 'none' ||
            manifestConfig.registration_requires_initial_access_token === true),
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

export function scopesForExistingSourceUserFlow(
    config: OAuthManifestConfig,
    sourceType: string,
    mode: 'read' | 'write',
    requiredScopes?: string[],
): string[] {
    const readScopes = config.scopes[sourceType]?.read ?? []
    if (mode === 'read') return readScopes

    const writeScopes = config.scopes[sourceType]?.write ?? []
    const requestedWriteScopes = requiredScopes?.length ? [...new Set(requiredScopes)] : writeScopes
    const invalidScopes = requestedWriteScopes.filter((scope) => !writeScopes.includes(scope))
    if (invalidScopes.length > 0) {
        throw new Error(
            `Unsupported write scopes for source_type=${sourceType}: ${invalidScopes.join(', ')}`,
        )
    }
    return [...new Set([...readScopes, ...requestedWriteScopes])]
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
    source?: Pick<Source, 'sourceType' | 'integrationType' | 'config'>
    userId: string
    returnTo?: string
}): Promise<{ url: string; requiredScopes: string[] }> {
    const manifestConfig = args.source
        ? await getOAuthConfigForSource(args.source)
        : await getOAuthManifestForSourceType(args.sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${args.sourceType}`)
    }
    if (manifestConfig.supports_org_oauth === false) {
        throw new Error(`OAuth for org sources is not supported by ${manifestConfig.provider}`)
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
    source?: Pick<Source, 'sourceType' | 'integrationType' | 'config'>
    userId: string
    returnTo?: string
    approvalId?: string
    approvalChatId?: string
    mode: 'read' | 'write'
    requiredScopes?: string[]
}): Promise<{ url: string; requiredScopes: string[] }> {
    const manifestConfig = args.source
        ? await getOAuthConfigForSource(args.source)
        : await getOAuthManifestForSourceType(args.sourceType)
    if (!manifestConfig) {
        throw new Error(`No OAuth manifest for source_type=${args.sourceType}`)
    }
    const creds = await loadClientCreds(manifestConfig.provider, manifestConfig)
    if (!creds) {
        throw new Error(`OAuth client not configured for provider=${manifestConfig.provider}`)
    }

    const actionScopes = scopesForExistingSourceUserFlow(
        manifestConfig,
        args.sourceType,
        args.mode,
        args.requiredScopes,
    )
    if (actionScopes.length === 0 && args.source?.integrationType !== IntegrationType.REMOTE_MCP) {
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
    source?: Pick<Source, 'sourceType' | 'integrationType' | 'config'>
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
    source?: Pick<Source, 'sourceType' | 'integrationType' | 'config'>
    userId: string
    returnTo?: string
    approvalId?: string
    approvalChatId?: string
    requiredScopes?: string[]
}): Promise<{ url: string; requiredScopes: string[] }> {
    return generateAuthUrlForExistingSourceUserFlow({ ...args, mode: 'write' })
}

function pkceForConfig(
    config: OAuthManifestConfig,
): { verifier: string; challenge: string } | null {
    if (!config.pkce_required && config.token_endpoint_auth_method !== 'none') {
        return null
    }
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
        state: stateToken,
        ...config.extra_auth_params,
    })
    params.set(config.scope_parameter ?? 'scope', scopes.join(config.scope_separator))
    if (codeChallenge) {
        params.set('code_challenge', codeChallenge)
        params.set('code_challenge_method', 'S256')
    }
    const authEndpoint = creds.authEndpoint ?? config.auth_endpoint
    return `${authEndpoint}?${params.toString()}`
}

interface SlackOAuthTokenResponse {
    ok?: boolean
    access_token?: string
    token_type?: string
    scope?: string
    refresh_token?: string
    expires_in?: number
    authed_user?: {
        id?: string
        access_token?: string
        token_type?: string
        scope?: string
        refresh_token?: string
        expires_in?: number
    }
}

export function normalizeOAuthTokens(
    provider: string,
    tokenData: OAuthTokens | SlackOAuthTokenResponse | OAuthError,
): OAuthTokens {
    if ('error' in tokenData) {
        throw new Error(`OAuth token exchange failed: ${tokenData.error}`)
    }
    if (provider === 'slack') {
        const slackData = tokenData as SlackOAuthTokenResponse
        const user = slackData.authed_user
        if (!user?.access_token) {
            throw new Error('Slack OAuth response did not contain a delegated user access token')
        }
        return {
            access_token: user.access_token,
            token_type: user.token_type ?? slackData.token_type ?? 'Bearer',
            scope: user.scope ?? slackData.scope,
            refresh_token: user.refresh_token ?? slackData.refresh_token,
            expires_in: user.expires_in ?? slackData.expires_in,
        }
    }
    if (!('access_token' in tokenData) || typeof tokenData.access_token !== 'string') {
        throw new Error('OAuth token response did not contain an access token')
    }
    return {
        access_token: tokenData.access_token,
        token_type: tokenData.token_type ?? 'Bearer',
        scope: tokenData.scope,
    }
}

export interface ExchangeResult {
    tokens: OAuthTokens
    state: ManifestOAuthState
    config: OAuthManifestConfig
    principalEmail: string
    clientCreds: ClientCreds
    userinfo?: unknown
}

export interface ExchangeCodeOptions {
    /// Identity for manifests without a userinfo endpoint. This must come from
    /// the already-authenticated Omni session, never from request input.
    authenticatedUserEmail?: string
}

/// Exchange an authorization code for tokens, validate state, and identify the
/// principal using the provider's userinfo endpoint or the authenticated Omni user.
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

    await validateRemoteMcpOAuthConfigUrls(config)
    const tokenEndpoint = creds.tokenEndpoint ?? config.token_endpoint
    if (requiresOAuthEndpointValidation(config.provider, config)) {
        await validateRemoteMcpUrlForCredentialUse(
            tokenEndpoint,
            ssrfPolicyForEndpoint(tokenEndpoint, windshiftInternalOrigin(config)),
        )
    }
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
    const tokenResp = await remoteMcpCredentialFetch(
        config.provider,
        tokenEndpoint,
        {
            method: 'POST',
            headers: tokenHeaders,
            body: tokenParams.toString(),
        },
        windshiftInternalOrigin(config),
        config.validate_endpoint_urls === true,
    )
    const tokenData = await readCredentialJson(config.provider, tokenResp).catch(() => null)
    if (!tokenResp.ok) {
        const oauthError = isOAuthError(tokenData)
        logger.warn('Connector OAuth token exchange failed', {
            provider: config.provider,
            flow: state.metadata.flow.type,
            status: tokenResp.status,
            tokenEndpoint,
            tokenEndpointAuthMethod: creds.tokenEndpointAuthMethod,
            clientIdPrefix: creds.clientId.slice(0, 12),
            resource: config.resource ?? null,
            error: oauthError ? tokenData.error : undefined,
            errorDescription: oauthError ? tokenData.error_description : undefined,
        })
        throw new Error(
            oauthError
                ? `OAuth token exchange failed: ${tokenData.error}${
                      tokenData.error_description ? ` - ${tokenData.error_description}` : ''
                  }`
                : 'OAuth token exchange failed with an invalid error response',
        )
    }
    const tokens = normalizeOAuthTokens(config.provider, tokenData)
    logger.info('Connector OAuth token exchange succeeded', {
        provider: config.provider,
        flow: state.metadata.flow.type,
        tokenType: tokens.token_type,
        expiresIn: tokens.expires_in,
        grantedScope: tokens.scope ?? null,
    })

    if (!config.userinfo_endpoint) {
        const authenticatedUserEmail = options.authenticatedUserEmail
        if (!authenticatedUserEmail) {
            throw new Error(
                `OAuth provider ${config.provider} does not advertise userinfo; an authenticated Omni user is required`,
            )
        }
        logger.info('Using authenticated Omni user as connector OAuth principal', {
            provider: config.provider,
            flow: state.metadata.flow.type,
        })
        return { tokens, state, config, principalEmail: authenticatedUserEmail, clientCreds: creds }
    }

    const userinfoResp = await remoteMcpCredentialFetch(
        config.provider,
        config.userinfo_endpoint,
        {
            headers: {
                Authorization: `Bearer ${tokens.access_token}`,
                Accept: 'application/json',
            },
        },
        windshiftInternalOrigin(config),
        config.validate_endpoint_urls === true,
    )
    if (!userinfoResp.ok) {
        const body = await readCredentialText(config.provider, userinfoResp).catch(() => '')
        logger.warn('Connector OAuth userinfo fetch failed', {
            provider: config.provider,
            flow: state.metadata.flow.type,
            status: userinfoResp.status,
            userinfoEndpoint: config.userinfo_endpoint,
            body: body.slice(0, 500),
        })
        throw new Error(`Failed to fetch userinfo: ${userinfoResp.status}`)
    }
    const profile = (await readCredentialJson(config.provider, userinfoResp)) as unknown
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

    return { tokens, state, config, principalEmail: email, clientCreds: creds, userinfo: profile }
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
    // Existing-source flows must reconstruct the source-specific OAuth
    // endpoints used during /oauth/start (not a provider-global default).
    if (flow.type !== 'connect_source') {
        const source = await getSourceById(flow.sourceId)
        return source ? getOAuthConfigForSource(source) : null
    }
    if (flow.type === 'connect_source' && flow.sourceTypes.length > 0) {
        return getOAuthManifestForSourceType(flow.sourceTypes[0])
    }
    return getOAuthManifestForProvider(provider)
}

async function getOAuthManifestForProvider(provider: string): Promise<OAuthManifestConfig | null> {
    const cfg = getConfig()
    const resp = await fetch(`${cfg.services.connectorManagerUrl}/connectors`)
    if (!resp.ok) return null
    const body: unknown = await resp.json().catch(() => null)
    if (!Array.isArray(body)) return null
    for (const entry of body) {
        const oauth = oauthManifestFromResponse(entry)
        if (oauth?.provider === provider) return oauth
    }
    return null
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function oauthManifestFromResponse(value: unknown): OAuthManifestConfig | null {
    if (!isRecord(value) || !isRecord(value.manifest)) return null
    const raw = value.manifest.oauth
    if (!isRecord(raw)) return null

    const provider = nonEmptyString(raw.provider)
    const authEndpoint = nonEmptyString(raw.auth_endpoint)
    const tokenEndpoint = nonEmptyString(raw.token_endpoint)
    if (!provider || !authEndpoint || !tokenEndpoint) return null

    const optionalStringKeys = [
        'userinfo_endpoint',
        'enrich_endpoint',
        'registration_endpoint',
        'resource',
        'credential_provider',
        'protected_resource_metadata_url',
        'authorization_server_metadata_url',
        'internal_base_url',
        'client_config_provider',
        'issuer_source_config_key',
        'client_config_provider_template',
    ]
    if (
        optionalStringKeys.some(
            (key) => raw[key] !== undefined && raw[key] !== null && typeof raw[key] !== 'string',
        )
    ) {
        return null
    }
    const optionalString = (key: string): string | null | undefined => {
        const field = raw[key]
        return field === undefined || field === null ? field : (field as string)
    }
    const optionalBoolean = (key: string, defaultValue: boolean): boolean | null => {
        const field = raw[key]
        if (field === undefined) return defaultValue
        return typeof field === 'boolean' ? field : null
    }
    const stringArray = (key: string): string[] | null => {
        const field = raw[key]
        if (field === undefined) return []
        if (!Array.isArray(field) || !field.every((item) => typeof item === 'string')) return null
        return field
    }
    const identityScopes = stringArray('identity_scopes')
    const tokenResponseFields = stringArray('token_response_fields')
    const grantTypes =
        raw.grant_types === null || raw.grant_types === undefined
            ? raw.grant_types
            : stringArray('grant_types')
    const userinfoEmailField = raw.userinfo_email_field ?? 'email'
    const scopeSeparator = raw.scope_separator ?? ' '
    if (
        identityScopes === null ||
        tokenResponseFields === null ||
        (grantTypes !== undefined && grantTypes !== null && !Array.isArray(grantTypes)) ||
        typeof userinfoEmailField !== 'string' ||
        !userinfoEmailField ||
        typeof scopeSeparator !== 'string' ||
        !scopeSeparator
    ) {
        return null
    }

    const scopes: Record<string, { read: string[]; write: string[] }> = {}
    const rawScopes = raw.scopes ?? {}
    if (!isRecord(rawScopes)) return null
    for (const [sourceType, rawScopeSet] of Object.entries(rawScopes)) {
        if (!isRecord(rawScopeSet)) return null
        const read = rawScopeSet.read ?? []
        const write = rawScopeSet.write ?? []
        if (
            !Array.isArray(read) ||
            !read.every((scope) => typeof scope === 'string') ||
            !Array.isArray(write) ||
            !write.every((scope) => typeof scope === 'string')
        ) {
            return null
        }
        scopes[sourceType] = { read, write }
    }

    const extraAuthParams: Record<string, string> = {}
    const rawExtraAuthParams = raw.extra_auth_params ?? {}
    if (!isRecord(rawExtraAuthParams)) return null
    for (const [key, param] of Object.entries(rawExtraAuthParams)) {
        if (typeof param !== 'string') return null
        extraAuthParams[key] = param
    }

    const tokenEndpointAuthMethod = raw.token_endpoint_auth_method ?? 'client_secret_post'
    if (!isOAuthTokenEndpointAuthMethod(tokenEndpointAuthMethod)) return null
    const registrationRequiresInitialAccessToken = optionalBoolean(
        'registration_requires_initial_access_token',
        false,
    )
    const pkceRequired = optionalBoolean('pkce_required', false)
    const validateEndpointUrls = optionalBoolean('validate_endpoint_urls', false)
    const supportsOrgOAuth = optionalBoolean('supports_org_oauth', true)
    if (
        registrationRequiresInitialAccessToken === null ||
        pkceRequired === null ||
        validateEndpointUrls === null ||
        supportsOrgOAuth === null
    ) {
        return null
    }

    return {
        provider,
        auth_endpoint: authEndpoint,
        token_endpoint: tokenEndpoint,
        userinfo_endpoint: optionalString('userinfo_endpoint'),
        userinfo_email_field: userinfoEmailField,
        identity_scopes: identityScopes,
        scopes,
        extra_auth_params: extraAuthParams,
        scope_separator: scopeSeparator,
        enrich_endpoint: optionalString('enrich_endpoint'),
        registration_endpoint: optionalString('registration_endpoint'),
        registration_requires_initial_access_token: registrationRequiresInitialAccessToken,
        token_response_fields: tokenResponseFields,
        token_endpoint_auth_method: tokenEndpointAuthMethod,
        resource: optionalString('resource'),
        credential_provider: optionalString('credential_provider'),
        protected_resource_metadata_url: optionalString('protected_resource_metadata_url'),
        authorization_server_metadata_url: optionalString('authorization_server_metadata_url'),
        internal_base_url: optionalString('internal_base_url'),
        client_config_provider: optionalString('client_config_provider'),
        issuer_source_config_key: optionalString('issuer_source_config_key'),
        client_config_provider_template: optionalString('client_config_provider_template'),
        pkce_required: pkceRequired,
        grant_types: grantTypes,
        validate_endpoint_urls: validateEndpointUrls,
        supports_org_oauth: supportsOrgOAuth,
    }
}

function nonEmptyString(value: unknown): string | null {
    return typeof value === 'string' && value.trim() ? value : null
}
