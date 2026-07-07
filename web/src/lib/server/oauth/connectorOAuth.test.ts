import { describe, expect, it } from 'vitest'
import {
    isAutoManagedOAuthProvider,
    isClientConfigComplete,
    tokenEndpointAuthMethodForConfig,
    type OAuthManifestConfig,
} from './connectorOAuth'

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
})
