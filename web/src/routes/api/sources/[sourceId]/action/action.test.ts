import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('$lib/server/config', () => ({
    getConfig: vi.fn(() => ({
        services: { connectorManagerUrl: 'http://cm.test' },
    })),
}))

vi.mock('$lib/server/logger', () => ({
    logger: { error: vi.fn() },
}))

vi.mock('$lib/server/repositories/sources', () => ({
    sourcesRepository: { getById: vi.fn() },
}))

const { POST } = await import('./+server')
const { sourcesRepository } = await import('$lib/server/repositories/sources')

function mockRequest(body: unknown): Request {
    return new Request('http://localhost/api/sources/source-1/action', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    })
}

async function callPost(
    body: unknown,
    user: { id: string; role: 'admin' | 'member' },
): Promise<{ status: number; body?: unknown }> {
    try {
        const response = await POST({
            params: { sourceId: 'source-1' },
            request: mockRequest(body),
            locals: { user },
        } as unknown as Parameters<typeof POST>[0])
        return { status: response.status, body: await response.json().catch(() => undefined) }
    } catch (err: unknown) {
        const httpError = err as { status?: number; body?: unknown }
        if (httpError.status !== undefined) {
            return { status: httpError.status, body: httpError.body }
        }
        throw err
    }
}

describe('POST /api/sources/[sourceId]/action', () => {
    afterEach(() => {
        vi.clearAllMocks()
        vi.unstubAllGlobals()
    })

    it('allows the owner to discover folders on their personal Drive source', async () => {
        vi.mocked(sourcesRepository.getById).mockResolvedValue({
            id: 'source-1',
            isDeleted: false,
            sourceType: 'google_drive',
            scope: 'user',
            createdBy: 'owner-1',
        } as never)
        vi.stubGlobal(
            'fetch',
            vi.fn().mockResolvedValue(
                new Response(JSON.stringify({ status: 'success', result: { items: [] } }), {
                    status: 200,
                    headers: { 'Content-Type': 'application/json' },
                }),
            ),
        )

        const response = await callPost(
            { action: 'discover_personal_folders', params: { query: 'roadmap' } },
            { id: 'owner-1', role: 'member' },
        )

        expect(response.status).toBe(200)
        expect(fetch).toHaveBeenCalledOnce()
    })

    it('denies non-owners, org sources, and admin discovery to members', async () => {
        const source = {
            id: 'source-1',
            isDeleted: false,
            sourceType: 'google_drive',
            scope: 'user',
            createdBy: 'owner-1',
        }
        vi.mocked(sourcesRepository.getById).mockResolvedValue(source as never)

        expect(
            (
                await callPost(
                    { action: 'discover_personal_folders' },
                    { id: 'member-2', role: 'member' },
                )
            ).status,
        ).toBe(403)
        vi.mocked(sourcesRepository.getById).mockResolvedValue({
            ...source,
            scope: 'org',
        } as never)
        expect(
            (
                await callPost(
                    { action: 'discover_personal_folders' },
                    { id: 'member-2', role: 'member' },
                )
            ).status,
        ).toBe(403)
        vi.mocked(sourcesRepository.getById).mockResolvedValue(source as never)
        vi.mocked(sourcesRepository.getById).mockClear()
        expect(
            (await callPost({ action: 'discover_folders' }, { id: 'owner-1', role: 'member' }))
                .status,
        ).toBe(403)
        expect(sourcesRepository.getById).not.toHaveBeenCalled()
    })

    it('allows admins to forward unrelated actions without a source lookup or folder validation', async () => {
        vi.stubGlobal(
            'fetch',
            vi.fn().mockResolvedValue(
                new Response(JSON.stringify({ status: 'success' }), {
                    status: 200,
                    headers: { 'Content-Type': 'application/json' },
                }),
            ),
        )

        const response = await callPost(
            { action: 'send_message', params: { query: 'x', unrelated: true } },
            { id: 'admin-1', role: 'admin' },
        )

        expect(response.status).toBe(200)
        expect(sourcesRepository.getById).not.toHaveBeenCalled()
        expect(fetch).toHaveBeenCalledOnce()
    })

    it('strictly validates persisted folder-discovery params without affecting other actions', async () => {
        vi.mocked(sourcesRepository.getById).mockResolvedValue({
            id: 'source-1',
            isDeleted: false,
            sourceType: 'google_drive',
            scope: 'user',
            createdBy: 'owner-1',
        } as never)
        const connectorFetch = vi.fn()
        vi.stubGlobal('fetch', connectorFetch)

        expect(
            (
                await callPost(
                    { action: 'discover_personal_folders' },
                    { id: 'owner-1', role: 'member' },
                )
            ).status,
        ).toBe(400)
        expect(
            (
                await callPost(
                    { action: 'discover_personal_folders', params: { query: 'x' } },
                    { id: 'owner-1', role: 'member' },
                )
            ).status,
        ).toBe(400)
        expect(
            (
                await callPost(
                    {
                        action: 'discover_personal_folders',
                        params: { query: 'roadmap', unrelated: true },
                    },
                    { id: 'owner-1', role: 'member' },
                )
            ).status,
        ).toBe(400)
        expect(connectorFetch).not.toHaveBeenCalled()
    })
})
