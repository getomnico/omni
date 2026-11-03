import { afterEach, describe, expect, it, vi } from 'vitest'

const { POST } = await import('./+server')

describe('POST /api/search/ai-answer', () => {
    afterEach(() => vi.restoreAllMocks())

    it('rejects anonymous requests before parsing JSON or calling searcher', async () => {
        const request = new Request('http://localhost/api/search/ai-answer', {
            method: 'POST',
            body: '{invalid',
        })
        const bodySpy = vi.spyOn(request, 'json')
        const fetch = vi.fn()
        let status: number | undefined

        try {
            await POST({
                request,
                fetch,
                locals: { user: null },
            } as unknown as Parameters<typeof POST>[0])
        } catch (err) {
            status = (err as { status?: number }).status
        }

        expect(status).toBe(401)
        expect(bodySpy).not.toHaveBeenCalled()
        expect(fetch).not.toHaveBeenCalled()
    })
})
