import { beforeEach, describe, expect, it, vi } from 'vitest'
import { OAuthStateManager } from './state'
import {
    dynamicRegistrationPayload,
    isAutoManagedOAuthProvider,
    isClientConfigComplete,
    oauthServiceBaseUrl,
    revokeDynamicallyRegisteredClient,
    scopesForExistingSourceUserFlow,
    tokenEndpointAuthMethodForConfig,
    windshiftInternalOrigin,
    type OAuthManifestConfig,
} from './connectorOAuth'

const { redisMock } = vi.hoisted(() => ({
    redisMock: {
        getDel: vi.fn(),
    },
}))

vi.mock('../redis', () => ({
    getRedisClient: vi.fn().mockResolvedValue(redisMock),
}))

const { validateRemoteMock, fetchRemoteMock } = vi.hoisted(() => ({
    validateRemoteMock: vi.fn(),
    fetchRemoteMock: vi.fn(),
}))

vi.mock('../mcp/client', async (importOriginal) => ({
    ...(await importOriginal()),
    validateRemoteMcpUrlForCredentialUse: validateRemoteMock,
    fetchWithPinnedRemoteMcpDns: fetchRemoteMock,
}))

const baseManifest: OAuthManifestConfig = {
    provider: 'example',
    auth_endpoint: 'https://example.com/oauth/authorize',
    token_endpoint: 'https://example.com/oauth/token',
    userinfo_endpoint: 'https://example.com/userinfo',
    userinfo_email_field: 'email',
    identity_scopes: [],
    scopes: { example: { read: ['read'], write: ['read', 'write'] } },
    extra_auth_params: {},
    scope_separator: ' ',
    token_endpoint_auth_method: 'client_secret_post',
}

describe('windshiftInternalOrigin', () => {
    const windshiftManifest: OAuthManifestConfig = {
        ...baseManifest,
        provider: 'windshift',
        auth_endpoint: 'https://windshift.example.com/oauth/authorize',
        token_endpoint: 'http://windshift:8080/api/oauth/token',
        userinfo_endpoint: 'http://windshift:8080/api/oauth/userinfo',
        internal_base_url: 'http://windshift:8080/',
    }

    it('returns the exact origin of the manifest internal route', () => {
        expect(windshiftInternalOrigin(windshiftManifest)).toBe('http://windshift:8080')
    })

    it('returns null when no internal route marker is advertised', () => {
        const publicOnly = { ...windshiftManifest }
        delete publicOnly.internal_base_url
        expect(windshiftInternalOrigin(publicOnly)).toBeNull()
    })

    it('returns null for non-windshift providers even with a marker', () => {
        expect(
            windshiftInternalOrigin({ ...baseManifest, internal_base_url: 'http://x:1' }),
        ).toBeNull()
    })

    it('returns null for an invalid internal URL', () => {
        expect(
            windshiftInternalOrigin({ ...windshiftManifest, internal_base_url: 'not a url' }),
        ).toBeNull()
    })
})

describe('OAuth connector helpers', () => {
    beforeEach(() => {
        vi.clearAllMocks()
    })

    it('atomically consumes OAuth state with GETDEL', async () => {
        redisMock.getDel.mockResolvedValueOnce(
            JSON.stringify({ id: 'state-1', state_token: 'state-1', provider: 'example' }),
        )

        await expect(OAuthStateManager.validateAndConsumeState('state-1')).resolves.toMatchObject({
            state_token: 'state-1',
        })
        expect(redisMock.getDel).toHaveBeenCalledOnce()
        expect(redisMock.getDel).toHaveBeenCalledWith('oauth_state:state-1')
    })

    it('infers auto-managed dynamic client registration providers from manifest fields', () => {
        expect(
            isAutoManagedOAuthProvider({
                ...baseManifest,
                registration_endpoint: 'https://example.com/oauth/register',
                token_endpoint_auth_method: 'none',
            }),
        ).toBe(true)

        expect(
            isAutoManagedOAuthProvider({
                ...baseManifest,
                registration_endpoint: 'https://example.com/oauth/register',
                token_endpoint_auth_method: 'client_secret_post',
            }),
        ).toBe(false)
    })

    it('builds provider-specific public-client registration metadata', () => {
        expect(
            dynamicRegistrationPayload(
                'windshift',
                'https://omni.example/api/oauth/callback',
                'mcp:access',
            ),
        ).toEqual({
            client_name: 'Omni Windshift MCP',
            redirect_uris: ['https://omni.example/api/oauth/callback'],
            grant_types: ['authorization_code', 'refresh_token'],
            response_types: ['code'],
            token_endpoint_auth_method: 'none',
            scope: 'mcp:access',
        })

        expect(
            dynamicRegistrationPayload(
                'clickup',
                'https://omni.example/api/oauth/callback',
                'tasks:read',
            ),
        ).toMatchObject({
            client_name: 'Omni ClickUp MCP',
            grant_types: ['authorization_code'],
        })

        expect(
            dynamicRegistrationPayload(
                'atlassian',
                'https://omni.example/api/oauth/callback',
                'read:jira-work',
            ),
        ).toMatchObject({
            client_name: 'Omni Atlassian MCP',
            grant_types: ['authorization_code', 'refresh_token'],
            token_endpoint_auth_method: 'none',
        })
    })

    it('checks configured state based on token endpoint auth method', () => {
        expect(isClientConfigComplete({ oauth_client_id: 'public-client' }, 'none')).toBe(true)
        expect(
            isClientConfigComplete(
                { oauth_client_id: 'confidential-client' },
                'client_secret_post',
            ),
        ).toBe(false)
        expect(
            isClientConfigComplete(
                {
                    oauth_client_id: 'confidential-client',
                    oauth_client_secret: 'secret',
                },
                'client_secret_basic',
            ),
        ).toBe(true)
    })

    it('uses typed stored token endpoint auth methods and falls back safely', () => {
        expect(tokenEndpointAuthMethodForConfig({ oauth_token_endpoint_auth_method: 'none' })).toBe(
            'none',
        )
        expect(
            tokenEndpointAuthMethodForConfig(
                { oauth_token_endpoint_auth_method: 'bogus' },
                { ...baseManifest, token_endpoint_auth_method: 'client_secret_basic' },
            ),
        ).toBe('client_secret_basic')
        expect(tokenEndpointAuthMethodForConfig(undefined, undefined)).toBe('client_secret_post')
    })

    it('derives a deployment base URL from its OAuth authorization endpoint', () => {
        expect(oauthServiceBaseUrl('https://windshift.example/oauth/authorize')).toBe(
            'https://windshift.example',
        )
        expect(
            oauthServiceBaseUrl('https://example.com/windshift/oauth/authorize?prompt=login'),
        ).toBe('https://example.com/windshift')
    })

    it('limits write elevation to the requested connector scopes', () => {
        const config: OAuthManifestConfig = {
            ...baseManifest,
            scopes: {
                windshift: {
                    read: ['mcp:access', 'items:read'],
                    write: ['mcp:access', 'items:read', 'items:write', 'items:delete'],
                },
            },
        }

        expect(
            scopesForExistingSourceUserFlow(config, 'windshift', 'write', ['items:write']),
        ).toEqual(['mcp:access', 'items:read', 'items:write'])
    })

    it('rejects requested scopes not declared by the connector', () => {
        expect(() =>
            scopesForExistingSourceUserFlow(baseManifest, 'example', 'write', ['unexpected:write']),
        ).toThrow('Unsupported write scopes')
    })
})

describe('revokeDynamicallyRegisteredClient', () => {
    const registrationConfig = {
        oauth_registration_client_uri:
            'https://login.salesforce.com/services/oauth2/register/abc123',
        oauth_registration_access_token: 'registration-token',
        oauth_registration_endpoint: 'https://login.salesforce.com/services/oauth2/register',
    }

    beforeEach(() => {
        vi.clearAllMocks()
        validateRemoteMock.mockImplementation(async (endpoint: string) => endpoint)
        fetchRemoteMock.mockResolvedValue(new Response(null, { status: 204 }))
    })

    it('does nothing when no registration metadata is stored', async () => {
        expect(await revokeDynamicallyRegisteredClient('salesforce:src-1', {})).toBe(false)
        expect(fetchRemoteMock).not.toHaveBeenCalled()
    })

    it('rejects a non-HTTPS client registration URI', async () => {
        const config = {
            ...registrationConfig,
            oauth_registration_client_uri:
                'http://login.salesforce.com/services/oauth2/register/abc',
        }
        expect(await revokeDynamicallyRegisteredClient('salesforce:src-1', config)).toBe(false)
        expect(fetchRemoteMock).not.toHaveBeenCalled()
    })

    it('rejects a client URI pointing at a different origin than the registration endpoint', async () => {
        const config = {
            ...registrationConfig,
            oauth_registration_client_uri: 'https://evil.example.com/services/oauth2/register/abc',
        }
        expect(await revokeDynamicallyRegisteredClient('salesforce:src-1', config)).toBe(false)
        expect(fetchRemoteMock).not.toHaveBeenCalled()
    })

    it('deletes the registered client with the bearer registration token', async () => {
        expect(
            await revokeDynamicallyRegisteredClient('salesforce:src-1', registrationConfig),
        ).toBe(true)
        expect(fetchRemoteMock).toHaveBeenCalledTimes(1)
        const [url, init] = fetchRemoteMock.mock.calls[0]
        expect(url.toString()).toBe(registrationConfig.oauth_registration_client_uri)
        expect(init.method).toBe('DELETE')
        expect(init.headers.Authorization).toBe('Bearer registration-token')
    })

    it('treats a 404 as already revoked', async () => {
        fetchRemoteMock.mockResolvedValue(new Response(null, { status: 404 }))
        expect(
            await revokeDynamicallyRegisteredClient('salesforce:src-1', registrationConfig),
        ).toBe(true)
    })

    it('fails closed when the provider rejects the deletion', async () => {
        fetchRemoteMock.mockResolvedValue(new Response(null, { status: 500 }))
        expect(
            await revokeDynamicallyRegisteredClient('salesforce:src-1', registrationConfig),
        ).toBe(false)
    })

    it('fails closed when the provider is unreachable', async () => {
        fetchRemoteMock.mockRejectedValue(new Error('network down'))
        expect(
            await revokeDynamicallyRegisteredClient('salesforce:src-1', registrationConfig),
        ).toBe(false)
    })
})
