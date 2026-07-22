import { beforeEach, describe, expect, it, vi } from 'vitest'
import { OAuthStateManager } from './state'
import {
    isAutoManagedOAuthProvider,
    isClientConfigComplete,
    scopesForExistingSourceUserFlow,
    tokenEndpointAuthMethodForConfig,
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

    it('limits write elevation to the requested connector scopes', () => {
        const config: OAuthManifestConfig = {
            ...baseManifest,
            scopes: {
                example: {
                    read: ['mcp:access', 'items:read'],
                    write: ['mcp:access', 'items:read', 'items:write', 'items:delete'],
                },
            },
        }

        expect(
            scopesForExistingSourceUserFlow(config, 'example', 'write', ['items:write']),
        ).toEqual(['mcp:access', 'items:read', 'items:write'])
    })

    it('rejects requested scopes not declared by the connector', () => {
        expect(() =>
            scopesForExistingSourceUserFlow(baseManifest, 'example', 'write', ['unexpected:write']),
        ).toThrow('Unsupported write scopes')
    })
})
