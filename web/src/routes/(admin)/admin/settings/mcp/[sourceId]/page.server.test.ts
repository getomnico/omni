import { describe, expect, it, vi } from 'vitest'
import { IntegrationType } from '$lib/types'
import { load } from './+page.server'

vi.mock('$lib/server/authHelpers', () => ({
    requireAdmin: vi.fn(),
}))

vi.mock('$lib/server/config', () => ({
    getConfig: () => ({ services: { connectorManagerUrl: 'http://connector-manager.test' } }),
}))

vi.mock('$lib/server/db/connector-configs', () => ({
    getConnectorConfigPublic: vi.fn(async (provider: string) => ({
        provider,
        config: { oauth_client_id: 'client-id' },
    })),
}))

describe('remote MCP admin edit page load', () => {
    it('uses the remote-MCP manifest identity when a native connector shares the same source_type', async () => {
        const fetchMock = vi.fn(async (url: string) => {
            if (url === '/api/remote-mcp/src-remote') {
                return new Response(
                    JSON.stringify({
                        id: 'src-remote',
                        name: 'Remote Docs',
                        sourceType: 'docs',
                        authType: 'oauth',
                        config: { endpoint_url: 'https://mcp.example.com/mcp' },
                        isActive: true,
                    }),
                    { status: 200 },
                )
            }
            if (url === 'http://connector-manager.test/connectors') {
                return new Response(
                    JSON.stringify([
                        {
                            source_type: 'docs',
                            manifest: {
                                integration_type: IntegrationType.CONNECTOR,
                                actions: [{ name: 'native_action' }],
                                resources: [{ name: 'native_resource' }],
                                oauth: { provider: 'native-docs' },
                            },
                        },
                        {
                            source_type: 'docs',
                            manifest: {
                                integration_type: IntegrationType.REMOTE_MCP,
                                actions: [{ name: 'remote_tool' }, { name: 'remote_write' }],
                                resources: [{ name: 'remote_resource' }],
                                oauth: {
                                    provider: 'remote_mcp:docs',
                                    token_endpoint_auth_method: 'none',
                                },
                            },
                        },
                    ]),
                    { status: 200 },
                )
            }
            throw new Error(`unexpected fetch ${url}`)
        })

        const result = (await load({
            locals: { user: { id: 'admin', role: 'admin' }, logger: { warn: vi.fn() } },
            params: { sourceId: 'src-remote' },
            fetch: fetchMock,
        } as never)) as any

        expect(result.manifest).toEqual({ available: true, toolCount: 2, resourceCount: 1 })
        expect(result.oauth.provider).toBe('remote_mcp:docs')
        expect(result.oauth.configured).toBe(true)
    })
})
