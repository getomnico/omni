import { error } from '@sveltejs/kit'
import { lookup } from 'node:dns/promises'
import net from 'node:net'
import { Agent } from 'undici'
import { AuthType } from '$lib/types'

export interface RemoteMcpConfig {
    endpoint_url: string
    auth_type?: AuthType.BEARER_TOKEN | AuthType.OAUTH | null
    write_tools_enabled: boolean
}

export interface RemoteMcpProbeOptions {
    endpointUrl: string
    authType?: AuthType.BEARER_TOKEN | AuthType.OAUTH | null
    bearerToken?: string | null
}

export interface RemoteMcpProbeResult {
    ok: boolean
    serverName: string | null
    serverVersion: string | null
    toolCount: number | null
    resourceCount: number | null
    oauth: Record<string, unknown> | null
    error?: string
}

const MAX_RESPONSE_BYTES = 1024 * 1024
const REMOTE_MCP_HTTP_TIMEOUT_MS = 20_000
const MCP_PROTOCOL_VERSION = '2024-11-05'

export interface RemoteMcpIpPolicy {
    /// Allow RFC1918 private IPv4 (10/8, 172.16/12, 192.168/16) destinations.
    /// Used for admin-declared internal endpoints (e.g. the Windshift internal
    /// URL) that are deliberately hosted on a private network. Loopback,
    /// link-local/metadata, and every other reserved range stay blocked.
    allowPrivate?: boolean
}

function isRfc1918Address(parts: number[]): boolean {
    const [a, b] = parts
    return a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168)
}

export function isRemoteMcpBlockedIp(address: string, policy: RemoteMcpIpPolicy = {}): boolean {
    const family = net.isIP(address)
    if (family === 4) {
        const parts = address.split('.').map((p) => Number.parseInt(p, 10))
        const [a, b, c] = parts
        if (policy.allowPrivate && isRfc1918Address(parts)) return false
        return (
            a === 0 ||
            a === 10 ||
            a === 127 ||
            (a === 100 && b >= 64 && b <= 127) ||
            (a === 169 && b === 254) ||
            (a === 172 && b >= 16 && b <= 31) ||
            (a === 192 && b === 0 && c === 0) ||
            (a === 192 && b === 0 && c === 2) ||
            (a === 192 && b === 88 && c === 99) ||
            (a === 192 && b === 168) ||
            (a === 198 && (b === 18 || b === 19)) ||
            (a === 198 && b === 51 && c === 100) ||
            (a === 203 && b === 0 && c === 113) ||
            a >= 224
        )
    }
    if (family === 6) {
        const normalized = address.toLowerCase()
        const mappedV4 = normalized.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/)?.[1]
        if (mappedV4) return isRemoteMcpBlockedIp(mappedV4)
        return (
            normalized === '::' ||
            normalized === '::1' ||
            normalized.startsWith('fc') ||
            normalized.startsWith('fd') ||
            normalized.startsWith('fe80:') ||
            normalized.startsWith('ff') ||
            normalized.startsWith('2001:db8:') ||
            normalized.startsWith('2002:')
        )
    }
    return true
}

export function validateRemoteMcpUrl(endpointUrl: string): URL {
    let url: URL
    try {
        url = new URL(endpointUrl)
    } catch {
        throw error(400, 'Invalid endpoint URL')
    }
    if (!['https:', 'http:'].includes(url.protocol)) {
        throw error(400, 'Endpoint URL must use http or https')
    }
    if (url.username || url.password) {
        throw error(400, 'Endpoint URL must not include credentials')
    }
    if (url.hash) {
        throw error(400, 'Endpoint URL must not include a fragment')
    }
    if (!url.hostname) {
        throw error(400, 'Endpoint URL must include a host')
    }
    return url
}

type PinnedAddress = { address: string; family: 4 | 6 }

async function resolveAllowedRemoteMcpAddresses(
    url: URL,
    policy: RemoteMcpIpPolicy = {},
): Promise<PinnedAddress[]> {
    const records = await lookup(url.hostname, { all: true, verbatim: true })
    if (records.length === 0) throw error(400, 'Endpoint host did not resolve')
    if (records.some((record) => isRemoteMcpBlockedIp(record.address, policy))) {
        throw error(400, 'Endpoint resolves to a disallowed network address')
    }
    return records.map((record) => ({ address: record.address, family: record.family as 4 | 6 }))
}

export async function assertRemoteMcpDestinationAllowed(
    url: URL,
    policy: RemoteMcpIpPolicy = {},
): Promise<void> {
    await resolveAllowedRemoteMcpAddresses(url, policy)
}

export async function validateRemoteMcpUrlForCredentialUse(
    endpointUrl: string,
    policy: RemoteMcpIpPolicy = {},
): Promise<string> {
    const url = validateRemoteMcpUrl(endpointUrl)
    await assertRemoteMcpDestinationAllowed(url, policy)
    return url.toString()
}

export async function fetchWithPinnedRemoteMcpDns(
    url: URL,
    init: RequestInit = {},
    policy: RemoteMcpIpPolicy = {},
): Promise<Response> {
    const addresses = await resolveAllowedRemoteMcpAddresses(url, policy)
    let next = 0
    const dispatcher = new Agent({
        connect: {
            lookup(
                _hostname: string,
                options: { all?: boolean },
                callback: (err: Error | null, result?: unknown) => void,
            ) {
                const record = addresses[next++ % addresses.length]
                if (options.all) {
                    callback(null, [{ address: record.address, family: record.family }])
                } else {
                    callback(null, record.address, record.family)
                }
            },
        },
    } as any)
    const timeoutSignal = AbortSignal.timeout(REMOTE_MCP_HTTP_TIMEOUT_MS)
    const signal = init.signal ? AbortSignal.any([init.signal, timeoutSignal]) : timeoutSignal
    try {
        const response = await fetch(url.toString(), {
            ...init,
            signal,
            redirect: 'manual',
            dispatcher,
        } as RequestInit & { dispatcher: unknown })
        const body = await readRemoteMcpLimitedBytes(response)
        return new Response(body, {
            status: response.status,
            statusText: response.statusText,
            headers: response.headers,
        })
    } finally {
        await dispatcher.close().catch(() => undefined)
    }
}

function parseSlugCandidate(name: string | null): string | null {
    if (!name) return null
    const slug = name
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '_')
        .replace(/^_+|_+$/g, '')
        .slice(0, 50)
    return /^[a-z][a-z0-9_-]{1,49}$/.test(slug) ? slug : null
}

export function validateRemoteMcpSlug(sourceType: string): string {
    if (!/^[a-z][a-z0-9_-]{1,49}$/.test(sourceType)) {
        throw error(
            400,
            'Invalid slug. Use 2-50 lowercase letters, numbers, hyphens, or underscores. Must start with a letter.',
        )
    }
    return sourceType
}

async function readRemoteMcpLimitedBytes(response: Response): Promise<Uint8Array> {
    const reader = response.body?.getReader()
    if (!reader) return new Uint8Array()
    const chunks: Uint8Array[] = []
    let total = 0
    while (true) {
        const { done, value } = await reader.read()
        if (done) break
        total += value.byteLength
        if (total > MAX_RESPONSE_BYTES) throw new Error('MCP response too large')
        chunks.push(value)
    }
    return Buffer.concat(chunks)
}

export async function readRemoteMcpLimitedJson(response: Response): Promise<any> {
    const body = new TextDecoder().decode(await readRemoteMcpLimitedBytes(response))
    if (!body.trim()) return null
    const eventData = body
        .split('\n')
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).trim())
        .find((line) => line && line !== '[DONE]')
    return JSON.parse(eventData ?? body)
}

function resourceMetadataUrlFromChallenge(response: Response): string | null {
    const challenge = response.headers.get('www-authenticate')
    if (!challenge) return null
    return /resource_metadata="([^"]+)"/.exec(challenge)?.[1] ?? null
}

function oauthFromChallenge(response: Response): Record<string, unknown> | null {
    const resourceMetadata = resourceMetadataUrlFromChallenge(response)
    if (!resourceMetadata) return null
    return {
        protected_resource_metadata_url: resourceMetadata,
    }
}

async function mcpPost(
    endpointUrl: string,
    headers: HeadersInit,
    sessionId: string | null,
    body: Record<string, unknown>,
): Promise<{ response: Response; json: any; sessionId: string | null }> {
    const url = validateRemoteMcpUrl(endpointUrl)

    const requestHeaders: Record<string, string> = {
        accept: 'application/json, text/event-stream',
        'content-type': 'application/json',
        ...Object.fromEntries(new Headers(headers).entries()),
    }
    if (sessionId) requestHeaders['mcp-session-id'] = sessionId

    const response = await fetchWithPinnedRemoteMcpDns(url, {
        method: 'POST',
        headers: requestHeaders,
        body: JSON.stringify(body),
    })
    const nextSessionId = response.headers.get('mcp-session-id') ?? sessionId
    const json = response.ok ? await readRemoteMcpLimitedJson(response) : null
    return { response, json, sessionId: nextSessionId }
}

function remoteMcpProbeErrorMessage(err: unknown): string {
    if (err instanceof Error) {
        const cause = err.cause
        if (cause instanceof Error) {
            return `${err.message}: ${cause.message}${cause.code ? ` (${cause.code})` : ''}`
        }
        if (typeof cause === 'string') return `${err.message}: ${cause}`
        return err.message
    }
    if (err && typeof err === 'object') {
        const body = 'body' in err ? (err as { body?: unknown }).body : undefined
        if (body && typeof body === 'object' && 'message' in body) {
            const message = (body as { message?: unknown }).message
            if (typeof message === 'string') return message
        }
        if ('message' in err && typeof (err as { message?: unknown }).message === 'string') {
            return (err as { message: string }).message
        }
    }
    return 'Remote MCP probe failed'
}

export async function probeRemoteMcpServer(
    options: RemoteMcpProbeOptions,
): Promise<RemoteMcpProbeResult & { suggestedSourceType: string | null }> {
    const url = validateRemoteMcpUrl(options.endpointUrl)
    await assertRemoteMcpDestinationAllowed(url)

    const authHeaders: Record<string, string> = {}
    if (options.authType === AuthType.BEARER_TOKEN && options.bearerToken) {
        authHeaders.authorization = `Bearer ${options.bearerToken}`
    }

    try {
        const init = await mcpPost(options.endpointUrl, authHeaders, null, {
            jsonrpc: '2.0',
            id: 1,
            method: 'initialize',
            params: {
                protocolVersion: MCP_PROTOCOL_VERSION,
                capabilities: {},
                clientInfo: { name: 'omni-web', version: '1.0.0' },
            },
        })

        if (init.response.status === 401 || init.response.status === 403) {
            return {
                ok: options.authType === AuthType.OAUTH,
                serverName: null,
                serverVersion: null,
                toolCount: null,
                resourceCount: null,
                oauth: oauthFromChallenge(init.response),
                suggestedSourceType: null,
                error: options.authType === AuthType.OAUTH ? undefined : 'Authorization required',
            }
        }
        if (!init.response.ok) {
            return {
                ok: false,
                serverName: null,
                serverVersion: null,
                toolCount: null,
                resourceCount: null,
                oauth: null,
                suggestedSourceType: null,
                error: `MCP initialize failed with HTTP ${init.response.status}`,
            }
        }

        const serverInfo = init.json?.result?.serverInfo ?? {}
        const sessionId = init.sessionId
        await mcpPost(options.endpointUrl, authHeaders, sessionId, {
            jsonrpc: '2.0',
            method: 'notifications/initialized',
            params: {},
        }).catch(() => null)

        const tools = await mcpPost(options.endpointUrl, authHeaders, sessionId, {
            jsonrpc: '2.0',
            id: 2,
            method: 'tools/list',
            params: {},
        }).catch(() => null)
        const resources = await mcpPost(options.endpointUrl, authHeaders, sessionId, {
            jsonrpc: '2.0',
            id: 3,
            method: 'resources/list',
            params: {},
        }).catch(() => null)
        const templates = await mcpPost(options.endpointUrl, authHeaders, sessionId, {
            jsonrpc: '2.0',
            id: 4,
            method: 'resources/templates/list',
            params: {},
        }).catch(() => null)

        const serverName = typeof serverInfo.name === 'string' ? serverInfo.name : null
        const serverVersion = typeof serverInfo.version === 'string' ? serverInfo.version : null
        const toolCount = Array.isArray(tools?.json?.result?.tools)
            ? tools.json.result.tools.length
            : 0
        const resourceCount =
            (Array.isArray(resources?.json?.result?.resources)
                ? resources.json.result.resources.length
                : 0) +
            (Array.isArray(templates?.json?.result?.resourceTemplates)
                ? templates.json.result.resourceTemplates.length
                : 0)

        return {
            ok: true,
            serverName,
            serverVersion,
            toolCount,
            resourceCount,
            oauth: null,
            suggestedSourceType: parseSlugCandidate(serverName),
        }
    } catch (err) {
        return {
            ok: false,
            serverName: null,
            serverVersion: null,
            toolCount: null,
            resourceCount: null,
            oauth: null,
            suggestedSourceType: null,
            error: remoteMcpProbeErrorMessage(err),
        }
    }
}

function wellKnownProtectedResourceUrls(endpointUrl: string): string[] {
    const endpoint = new URL(endpointUrl)
    const urls = [`${endpoint.origin}/.well-known/oauth-protected-resource`]
    if (endpoint.pathname && endpoint.pathname !== '/') {
        urls.unshift(`${endpoint.origin}/.well-known/oauth-protected-resource${endpoint.pathname}`)
    }
    return [...new Set(urls)]
}

async function fetchAllowedJson(url: string): Promise<any | null> {
    const parsed = validateRemoteMcpUrl(url)
    const response = await fetchWithPinnedRemoteMcpDns(parsed, {
        method: 'GET',
        headers: { accept: 'application/json' },
    })
    if (!response.ok) return null
    return readRemoteMcpLimitedJson(response)
}

async function protectedResourceMetadataUrl(endpointUrl: string): Promise<string | null> {
    const init = await mcpPost(endpointUrl, {}, null, {
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
            protocolVersion: MCP_PROTOCOL_VERSION,
            capabilities: {},
            clientInfo: { name: 'omni-web', version: '1.0.0' },
        },
    }).catch(() => null)
    const challenged = init ? resourceMetadataUrlFromChallenge(init.response) : null
    if (challenged) return challenged

    for (const candidate of wellKnownProtectedResourceUrls(endpointUrl)) {
        if (await fetchAllowedJson(candidate)) return candidate
    }
    return null
}

function authServerMetadataUrls(issuerOrMetadata: string): string[] {
    const url = validateRemoteMcpUrl(issuerOrMetadata)
    if (url.pathname.includes('/.well-known/oauth-authorization-server')) {
        return [url.toString()]
    }
    return [
        `${url.origin}/.well-known/oauth-authorization-server${url.pathname === '/' ? '' : url.pathname}`,
        `${url.origin}/.well-known/openid-configuration${url.pathname === '/' ? '' : url.pathname}`,
    ]
}

export async function discoverRemoteMcpOAuthConfig(args: {
    endpointUrl: string
    sourceType: string
}): Promise<Record<string, unknown> | null> {
    const endpoint = validateRemoteMcpUrl(args.endpointUrl)
    await assertRemoteMcpDestinationAllowed(endpoint)

    const prmUrl = await protectedResourceMetadataUrl(endpoint.toString())
    if (!prmUrl) return null
    const prm = await fetchAllowedJson(prmUrl)
    if (!prm || typeof prm !== 'object') return null

    const authServers = Array.isArray(prm.authorization_servers)
        ? prm.authorization_servers.filter(
              (value: unknown): value is string => typeof value === 'string',
          )
        : []
    if (authServers.length === 0) return null

    for (const authServer of authServers) {
        for (const metadataUrl of authServerMetadataUrls(authServer)) {
            const asMetadata = await fetchAllowedJson(metadataUrl)
            if (!asMetadata || typeof asMetadata !== 'object') continue
            if (
                typeof asMetadata.authorization_endpoint !== 'string' ||
                typeof asMetadata.token_endpoint !== 'string'
            ) {
                continue
            }
            const authEndpoint = await validateRemoteMcpUrlForCredentialUse(
                asMetadata.authorization_endpoint,
            ).catch(() => null)
            const tokenEndpoint = await validateRemoteMcpUrlForCredentialUse(
                asMetadata.token_endpoint,
            ).catch(() => null)
            if (!authEndpoint || !tokenEndpoint) continue
            const userinfoEndpoint =
                typeof asMetadata.userinfo_endpoint === 'string'
                    ? await validateRemoteMcpUrlForCredentialUse(
                          asMetadata.userinfo_endpoint,
                      ).catch(() => null)
                    : endpoint.origin
            const registrationEndpoint =
                typeof asMetadata.registration_endpoint === 'string'
                    ? await validateRemoteMcpUrlForCredentialUse(
                          asMetadata.registration_endpoint,
                      ).catch(() => null)
                    : null
            const resource =
                typeof prm.resource === 'string'
                    ? await validateRemoteMcpUrlForCredentialUse(prm.resource).catch(() => null)
                    : endpoint.toString()
            if (
                !userinfoEndpoint ||
                (asMetadata.registration_endpoint && !registrationEndpoint) ||
                !resource
            ) {
                continue
            }
            const scopes = Array.isArray(asMetadata.scopes_supported)
                ? asMetadata.scopes_supported.filter(
                      (value: unknown): value is string => typeof value === 'string',
                  )
                : []
            const tokenAuthMethods = Array.isArray(asMetadata.token_endpoint_auth_methods_supported)
                ? asMetadata.token_endpoint_auth_methods_supported
                : []
            return {
                provider: `remote_mcp:${args.sourceType}`,
                credential_provider: 'remote_mcp',
                auth_endpoint: authEndpoint,
                token_endpoint: tokenEndpoint,
                userinfo_endpoint: userinfoEndpoint,
                userinfo_email_field: 'email',
                identity_scopes: [],
                scopes: { [args.sourceType]: { read: scopes, write: scopes } },
                extra_auth_params: {},
                scope_separator: ' ',
                registration_endpoint: registrationEndpoint,
                token_endpoint_auth_method: tokenAuthMethods.includes('none')
                    ? 'none'
                    : 'client_secret_post',
                resource,
                protected_resource_metadata_url: prmUrl,
                authorization_server_metadata_url: metadataUrl,
            }
        }
    }

    return null
}

export function remoteMcpConfigFromInput(input: {
    endpointUrl: string
    authType?: string | null
    writeToolsEnabled?: boolean
}): RemoteMcpConfig {
    const url = validateRemoteMcpUrl(input.endpointUrl)
    const authType = input.authType ?? null
    if (authType !== null && authType !== AuthType.BEARER_TOKEN && authType !== AuthType.OAUTH) {
        throw error(400, 'authType must be null, bearer_token, or oauth')
    }
    return {
        endpoint_url: url.toString(),
        auth_type: authType,
        write_tools_enabled: input.writeToolsEnabled ?? true,
    }
}
