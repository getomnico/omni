import { describe, expect, it, vi, beforeEach } from 'vitest'

// Mock server dependencies before importing the handler.
vi.mock('$lib/server/config', () => ({
    getConfig: vi.fn(() => ({
        database: { url: 'postgresql://test:test@localhost:5432/test' },
        redis: { url: 'redis://localhost:6379' },
        services: {
            searcherUrl: 'http://searcher.test',
            indexerUrl: 'http://indexer.test',
            aiServiceUrl: 'http://ai.test',
            connectorManagerUrl: 'http://cm.test',
        },
        session: { secret: 'test-secret', cookieName: 'test-cookie', durationDays: 30 },
        app: { publicUrl: 'http://localhost:3000' },
    })),
}))

vi.mock('$lib/server/logger', () => ({
    logger: { error: vi.fn() },
}))

const { POST } = await import('./+server')

function mockRequest(body: unknown, headers: Record<string, string> = {}): Request {
    return new Request('http://localhost/api/preview-action', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...headers },
        body: JSON.stringify(body),
    })
}

function mockLocals(): Record<string, unknown> {
    return { user: { id: 'admin-1', role: 'admin' } }
}

/**
 * Helper that calls POST and converts both success and thrown HttpError
 * into a consistent { status, body? } shape so tests don't need .catch().
 */
async function callPost(
    body: unknown,
    locals?: Record<string, unknown>,
): Promise<{ status: number; body?: unknown }> {
    try {
        const response = await POST({
            request: mockRequest(body),
            locals: locals ?? mockLocals(),
        } as unknown as Parameters<typeof POST>[0])
        return { status: response.status, body: await response.json().catch(() => undefined) }
    } catch (err: unknown) {
        // SvelteKit error() throws HttpError { status, body }
        const httpError = err as { status?: number; body?: unknown }
        if (httpError.status !== undefined) {
            return { status: httpError.status, body: httpError.body }
        }
        throw err
    }
}

describe('POST /api/preview-action', () => {
    beforeEach(() => {
        vi.restoreAllMocks()
    })

    it('rejects non-admin users with 403', async () => {
        const response = await callPost(
            {
                sourceType: 'google_drive',
                action: 'discover_folders',
                serviceAccountJson: '{}',
                principalEmail: 'a@b.com',
                domain: 'b.com',
            },
            { user: null },
        )
        expect(response.status).toBe(401)
    })

    it('rejects non-admin role with 403', async () => {
        const response = await callPost(
            {
                sourceType: 'google_drive',
                action: 'discover_folders',
                serviceAccountJson: '{}',
                principalEmail: 'a@b.com',
                domain: 'b.com',
            },
            { user: { id: 'u1', role: 'member' } },
        )
        expect(response.status).toBe(403)
    })

    it('rejects unknown top-level fields', async () => {
        const response = await callPost({
            sourceType: 'google_drive',
            action: 'discover_folders',
            unknownField: 'x',
            serviceAccountJson: '{}',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        })
        expect(response.status).toBe(400)
        const body = response.body as { message?: string } | undefined
        expect(body?.message || '').toContain('Unknown field')
    })

    it('rejects unsupported action', async () => {
        const response = await callPost({
            sourceType: 'google_drive',
            action: 'delete_all_files',
            serviceAccountJson: '{}',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        })
        expect(response.status).toBe(400)
    })

    it('rejects non-google_drive sourceType', async () => {
        const response = await callPost({
            sourceType: 'gmail',
            action: 'discover_folders',
            serviceAccountJson: '{}',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        })
        expect(response.status).toBe(400)
    })

    it('rejects missing required credential fields', async () => {
        const response = await callPost({
            sourceType: 'google_drive',
            action: 'discover_folders',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        })
        expect(response.status).toBe(400)
    })

    it('rejects invalid service account JSON', async () => {
        const response = await callPost({
            sourceType: 'google_drive',
            action: 'discover_folders',
            serviceAccountJson: '{invalid}',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        })
        expect(response.status).toBe(400)
    })
})
