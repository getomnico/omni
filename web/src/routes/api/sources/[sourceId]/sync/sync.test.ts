import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('$lib/server/repositories/sources', () => ({
    sourcesRepository: { getById: vi.fn() },
}))
vi.mock('$lib/server/config', () => ({
    getConfig: () => ({ services: { connectorManagerUrl: 'http://connector.test' } }),
}))
vi.mock('$lib/server/logger', () => ({ logger: { error: vi.fn() } }))

const { POST } = await import('./+server')
const { sourcesRepository } = await import('$lib/server/repositories/sources')

function makeEvent(user: { id: string; role: 'admin' | 'member' } | null, fetch = vi.fn()) {
    return {
        params: { sourceId: 'source-1' },
        request: new Request('http://localhost/api/sources/source-1/sync', {
            method: 'POST',
            body: '{invalid',
        }),
        fetch,
        locals: { user },
    } as unknown as Parameters<typeof POST>[0]
}

async function statusOf(call: Promise<unknown>): Promise<number | undefined> {
    try {
        const result = await call
        return (result as Response).status
    } catch (err) {
        return (err as { status?: number }).status
    }
}

describe('POST /api/sources/[sourceId]/sync', () => {
    afterEach(() => {
        vi.clearAllMocks()
        vi.unstubAllGlobals()
    })

    it('rejects anonymous requests before body parsing or downstream work', async () => {
        const ctx = makeEvent(null)
        const bodySpy = vi.spyOn(ctx.request, 'json')

        expect(await statusOf(POST(ctx))).toBe(401)
        expect(bodySpy).not.toHaveBeenCalled()
        expect(sourcesRepository.getById).not.toHaveBeenCalled()
        expect(ctx.fetch).not.toHaveBeenCalled()
    })

    it('rejects non-owner members from triggering source sync', async () => {
        vi.mocked(sourcesRepository.getById).mockResolvedValue({
            id: 'source-1',
            isDeleted: false,
            createdBy: 'owner-1',
            scope: 'user',
            integrationType: 'google_drive',
            config: {},
        } as never)
        const ctx = makeEvent({ id: 'other-user', role: 'member' })

        expect(await statusOf(POST(ctx))).toBe(403)
        expect(ctx.fetch).not.toHaveBeenCalled()
    })
})
