import { afterEach, describe, expect, it, vi } from 'vitest'

const { POST } = await import('./+server')

describe('POST /api/v1/search admin API-key authorization', () => {
    afterEach(() => vi.restoreAllMocks())

    it('marks authorized admin-key searches explicitly without accepting body elevation', async () => {
        const fetch = vi
            .fn()
            .mockResolvedValue(
                new Response(JSON.stringify({ results: [], total_count: 0 }), { status: 200 }),
            )
        const response = await POST({
            request: new Request('http://localhost/api/v1/search', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ query: 'all docs', admin_access: false }),
            }),
            fetch,
            locals: {
                user: { id: 'key-user', email: 'admin@example.com', configuration: {} },
                apiKeyScope: 'admin',
                apiKeyAllowedSources: ['google_drive'],
                logger: { child: () => ({ debug: vi.fn(), error: vi.fn(), info: vi.fn() }) },
            },
        } as unknown as Parameters<typeof POST>[0])

        expect(response.status).toBe(200)
        const [, init] = fetch.mock.calls[0]
        expect(JSON.parse(String(init.body))).toMatchObject({
            admin_access: true,
            source_types: ['google_drive'],
        })
        expect(JSON.parse(String(init.body))).not.toHaveProperty('user_email')
    })
})
