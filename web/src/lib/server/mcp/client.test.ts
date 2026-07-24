import { describe, expect, it } from 'vitest'
import { AuthType } from '$lib/types'
import {
    isRemoteMcpBlockedIp,
    remoteMcpConfigFromInput,
    validateRemoteMcpSlug,
    validateRemoteMcpUrl,
    validateRemoteMcpUrlForCredentialUse,
} from './client'

describe('remote MCP client validation', () => {
    it('blocks private and reserved addresses used in SSRF attempts', () => {
        expect(isRemoteMcpBlockedIp('127.0.0.1')).toBe(true)
        expect(isRemoteMcpBlockedIp('10.1.2.3')).toBe(true)
        expect(isRemoteMcpBlockedIp('172.16.0.1')).toBe(true)
        expect(isRemoteMcpBlockedIp('192.168.1.10')).toBe(true)
        expect(isRemoteMcpBlockedIp('169.254.169.254')).toBe(true)
        expect(isRemoteMcpBlockedIp('198.18.0.1')).toBe(true)
        expect(isRemoteMcpBlockedIp('203.0.113.10')).toBe(true)
        expect(isRemoteMcpBlockedIp('::1')).toBe(true)
        expect(isRemoteMcpBlockedIp('fc00::1')).toBe(true)
        expect(isRemoteMcpBlockedIp('fe80::1')).toBe(true)
        expect(isRemoteMcpBlockedIp('::ffff:127.0.0.1')).toBe(true)
    })

    it('allows public routable addresses', () => {
        expect(isRemoteMcpBlockedIp('8.8.8.8')).toBe(false)
        expect(isRemoteMcpBlockedIp('1.1.1.1')).toBe(false)
        expect(isRemoteMcpBlockedIp('2001:4860:4860::8888')).toBe(false)
    })

    it('rejects URL credentials, fragments, and unsupported schemes', () => {
        expect(() => validateRemoteMcpUrl('file:///tmp/server')).toThrow()
        expect(() => validateRemoteMcpUrl('https://user:pass@example.com/mcp')).toThrow()
        expect(() => validateRemoteMcpUrl('https://example.com/mcp#fragment')).toThrow()
    })

    it('parses allowed config using existing AuthType values', () => {
        expect(
            remoteMcpConfigFromInput({
                endpointUrl: 'https://example.com/mcp',
                authType: AuthType.BEARER_TOKEN,
                writeToolsEnabled: false,
            }),
        ).toEqual({
            endpoint_url: 'https://example.com/mcp',
            auth_type: AuthType.BEARER_TOKEN,
            write_tools_enabled: false,
        })
        expect(() =>
            remoteMcpConfigFromInput({
                endpointUrl: 'https://example.com/mcp',
                authType: 'api_key',
            }),
        ).toThrow()
    })

    it('rejects OAuth metadata-derived credential URLs resolving to blocked addresses', async () => {
        await expect(
            validateRemoteMcpUrlForCredentialUse('http://127.0.0.1/token'),
        ).rejects.toThrow()
    })

    it('validates immutable source slug shape', () => {
        expect(validateRemoteMcpSlug('github_mcp')).toBe('github_mcp')
        expect(() => validateRemoteMcpSlug('Remote-MCP')).toThrow()
        expect(() => validateRemoteMcpSlug('1github')).toThrow()
    })
})
