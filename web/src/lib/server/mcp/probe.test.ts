import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AuthType } from '$lib/types'

const { lookupMock, fetchMock } = vi.hoisted(() => ({
    lookupMock: vi.fn(),
    fetchMock: vi.fn(),
}))

vi.mock('node:dns/promises', () => ({
    lookup: lookupMock,
}))

function jsonRpcResponse(body: unknown, headers?: Record<string, string>) {
    return new Response(JSON.stringify(body), {
        status: 200,
        headers: { 'content-type': 'application/json', ...headers },
    })
}

describe('remote MCP probe network behavior', () => {
    beforeEach(() => {
        vi.resetModules()
        vi.clearAllMocks()
        lookupMock.mockResolvedValue([{ address: '8.8.8.8', family: 4 }])
        fetchMock.mockReset()
        vi.stubGlobal('fetch', fetchMock)
    })

    it('uses bearer auth, disables redirects, and re-checks DNS before each MCP request', async () => {
        const { probeRemoteMcpServer } = await import('./client')
        fetchMock
            .mockResolvedValueOnce(
                jsonRpcResponse(
                    {
                        jsonrpc: '2.0',
                        result: { serverInfo: { name: 'Acme Docs', version: '1.0.0' } },
                    },
                    { 'mcp-session-id': 'session-1' },
                ),
            )
            .mockResolvedValueOnce(jsonRpcResponse({ jsonrpc: '2.0', result: {} }))
            .mockResolvedValueOnce(
                jsonRpcResponse({ jsonrpc: '2.0', result: { tools: [{ name: 'search' }] } }),
            )
            .mockResolvedValueOnce(
                jsonRpcResponse({
                    jsonrpc: '2.0',
                    result: { resources: [{ uri: 'docs://guide' }] },
                }),
            )
            .mockResolvedValueOnce(
                jsonRpcResponse({ jsonrpc: '2.0', result: { resourceTemplates: [] } }),
            )

        const result = await probeRemoteMcpServer({
            endpointUrl: 'https://mcp.example.com/mcp',
            authType: AuthType.BEARER_TOKEN,
            bearerToken: 'secret-token',
        })

        expect(result).toMatchObject({
            ok: true,
            serverName: 'Acme Docs',
            serverVersion: '1.0.0',
            toolCount: 1,
            resourceCount: 1,
            suggestedSourceType: 'acme_docs',
        })
        expect(lookupMock).toHaveBeenCalledTimes(6)
        expect(fetchMock).toHaveBeenCalledWith(
            'https://mcp.example.com/mcp',
            expect.objectContaining({
                method: 'POST',
                redirect: 'manual',
                signal: expect.any(AbortSignal),
                headers: expect.objectContaining({
                    authorization: 'Bearer secret-token',
                    'mcp-session-id': 'session-1',
                }),
            }),
        )
    })

    it('treats OAuth authorization challenges as a successful setup probe with normalized metadata', async () => {
        const { probeRemoteMcpServer } = await import('./client')
        fetchMock.mockResolvedValueOnce(
            new Response('', {
                status: 401,
                headers: {
                    'www-authenticate':
                        'Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource"',
                },
            }),
        )

        const result = await probeRemoteMcpServer({
            endpointUrl: 'https://mcp.example.com/mcp',
            authType: AuthType.OAUTH,
        })

        expect(result.ok).toBe(true)
        expect(result.oauth).toEqual({
            protected_resource_metadata_url:
                'https://mcp.example.com/.well-known/oauth-protected-resource',
        })
        expect(result.error).toBeUndefined()
    })

    it('fails closed when DNS rebinding changes the resolved address before an MCP request', async () => {
        const { probeRemoteMcpServer } = await import('./client')
        lookupMock
            .mockResolvedValueOnce([{ address: '8.8.8.8', family: 4 }])
            .mockResolvedValueOnce([{ address: '127.0.0.1', family: 4 }])

        const result = await probeRemoteMcpServer({ endpointUrl: 'https://mcp.example.com/mcp' })

        expect(result.ok).toBe(false)
        expect(result.error).toContain('Endpoint resolves to a disallowed network address')
        expect(fetchMock).not.toHaveBeenCalled()
    })
})
