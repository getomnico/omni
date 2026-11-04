import { afterEach, describe, expect, it, vi } from 'vitest'

const { POST } = await import('./+server')

const user = {
    id: 'user-1',
    email: 'user@example.com',
    configuration: {},
}

function event(request: Request, authenticated = true) {
    return {
        request,
        fetch: vi
            .fn()
            .mockResolvedValue(new Response(JSON.stringify({ results: [] }), { status: 200 })),
        locals: {
            user: authenticated ? user : null,
            logger: {
                child: () => ({ warn: vi.fn(), debug: vi.fn(), info: vi.fn(), error: vi.fn() }),
            },
        },
    } as unknown as Parameters<typeof POST>[0]
}

describe('POST /api/search', () => {
    afterEach(() => vi.restoreAllMocks())

    it('rejects anonymous requests before parsing JSON or calling searcher', async () => {
        const request = new Request('http://localhost/api/search', {
            method: 'POST',
            body: '{invalid',
        })
        const bodySpy = vi.spyOn(request, 'json')
        const ctx = event(request, false)
        const response = await POST(ctx)

        expect(response.status).toBe(401)
        expect(bodySpy).not.toHaveBeenCalled()
        expect(ctx.fetch).not.toHaveBeenCalled()
    })

    it('forwards authenticated identity to preserve user-scoped search', async () => {
        const ctx = event(
            new Request('http://localhost/api/search', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ query: 'secret' }),
            }),
        )
        await POST(ctx)

        expect(ctx.fetch).toHaveBeenCalledOnce()
        const [, init] = vi.mocked(ctx.fetch).mock.calls[0]
        expect(JSON.parse(String(init?.body))).toMatchObject({
            user_id: user.id,
            user_email: user.email,
            query: 'secret',
        })
    })
})
