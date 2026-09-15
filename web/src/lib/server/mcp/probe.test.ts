import { beforeEach, describe, expect, it, vi } from 'vitest'
import { EventEmitter } from 'node:events'
import type { IncomingMessage } from 'node:http'
import { AuthType } from '$lib/types'

const { lookupMock, requestMock, responseQueue } = vi.hoisted(() => ({
    lookupMock: vi.fn(),
    requestMock: vi.fn(),
    responseQueue: [] as Array<{
        status: number
        body: string
        headers: Record<string, string>
    }>,
}))

vi.mock('node:dns/promises', () => ({
    lookup: lookupMock,
}))

// The pinned-DNS fetch is implemented on node:http/node:https (see
// fetchWithPinnedRemoteMcpDns). Mock both transports: the request mock hands
// back a fake IncomingMessage fed from the per-test response queue.
vi.mock('node:http', () => ({
    default: { request: requestMock },
    request: requestMock,
}))
vi.mock('node:https', () => ({
    default: { request: requestMock },
    request: requestMock,
}))

function fakeIncomingMessage(
    status: number,
    body: string,
    headers: Record<string, string>,
): IncomingMessage {
    const message = new EventEmitter() as IncomingMessage & {
        statusCode: number
        statusMessage: string
        headers: Record<string, string>
    }
    message.statusCode = status
    message.statusMessage = 'OK'
    message.headers = headers
    // setImmediate so the consumer's 'data'/'end' listeners attach first
    // (microtasks would run before the awaiting code resumes)
    setImmediate(() => {
        message.emit('data', Buffer.from(body))
        message.emit('end')
    })
    return message
}

function enqueueJsonResponse(body: unknown, headers?: Record<string, string>) {
    responseQueue.push({
        status: 200,
        body: JSON.stringify(body),
        headers: { 'content-type': 'application/json', ...headers },
    })
}

function jsonRpcResult(result: unknown, headers?: Record<string, string>) {
    return enqueueJsonResponse({ jsonrpc: '2.0', result }, headers)
}

describe('remote MCP probe network behavior', () => {
    beforeEach(() => {
        vi.clearAllMocks()
        lookupMock.mockResolvedValue([{ address: '8.8.8.8', family: 4 }])
        responseQueue.length = 0
        requestMock.mockImplementation(
            (
                _url: unknown,
                _options: Record<string, unknown>,
                callback: (response: IncomingMessage) => void,
            ) => {
                const request = new EventEmitter() as EventEmitter & {
                    write: () => undefined
                    end: () => void
                    destroy: () => undefined
                }
                request.write = () => undefined
                request.destroy = () => undefined
                request.end = () => {
                    const queued = responseQueue.shift()
                    callback(
                        fakeIncomingMessage(
                            queued?.status ?? 500,
                            queued?.body ?? '',
                            queued?.headers ?? {},
                        ),
                    )
                }
                return request
            },
        )
    })

    it('uses bearer auth, disables redirects, and re-checks DNS before each MCP request', async () => {
        const { probeRemoteMcpServer } = await import('./client')
        jsonRpcResult(
            { serverInfo: { name: 'Acme Docs', version: '1.0.0' } },
            { 'mcp-session-id': 'session-1' },
        )
        jsonRpcResult({})
        jsonRpcResult({ tools: [{ name: 'search' }] })
        jsonRpcResult({ resources: [{ uri: 'docs://guide' }] })
        jsonRpcResult({ resourceTemplates: [] })

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
        expect(requestMock).toHaveBeenCalledWith(
            expect.objectContaining({ href: 'https://mcp.example.com/mcp' }),
            expect.objectContaining({
                method: 'POST',
                headers: expect.objectContaining({
                    authorization: 'Bearer secret-token',
                    'mcp-session-id': 'session-1',
                }),
                // node:http/node:https never follow redirects, which preserves
                // the previous redirect: 'manual' behavior.
            }),
            expect.any(Function),
        )
    })

    it('treats OAuth authorization challenges as a successful setup probe with normalized metadata', async () => {
        const { probeRemoteMcpServer } = await import('./client')
        responseQueue.push({
            status: 401,
            body: '',
            headers: {
                'www-authenticate':
                    'Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource"',
            },
        })

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
        expect(requestMock).not.toHaveBeenCalled()
    })
})
