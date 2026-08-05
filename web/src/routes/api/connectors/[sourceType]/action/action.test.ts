import { afterEach, describe, expect, it, vi } from 'vitest'

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
    return new Request('http://localhost/api/connectors/google_drive/action', {
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
    sourceType = 'google_drive',
): Promise<{ status: number; body?: unknown }> {
    try {
        const response = await POST({
            params: { sourceType },
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

describe('POST /api/connectors/[sourceType]/action', () => {
    afterEach(() => {
        vi.unstubAllGlobals()
    })

    it('requires an admin', async () => {
        const request = {
            action: 'discover_folders',
            serviceAccountJson: '{}',
            principalEmail: 'a@b.com',
            domain: 'b.com',
        }

        expect((await callPost(request, { user: null })).status).toBe(401)
        expect((await callPost(request, { user: { id: 'u1', role: 'member' } })).status).toBe(403)
    })

    it('rejects connector source types without an allowlisted transient action', async () => {
        const response = await callPost(
            {
                action: 'discover_folders',
                serviceAccountJson: '{}',
                principalEmail: 'a@b.com',
                domain: 'b.com',
            },
            undefined,
            'slack',
        )

        expect(response.status).toBe(400)
    })

    it('forwards validated setup credentials through the normal action endpoint', async () => {
        const connectorResponse = {
            status: 'success',
            result: { items: [] },
        }
        const connectorFetch = vi.fn().mockResolvedValue(
            new Response(JSON.stringify(connectorResponse), {
                status: 200,
                headers: { 'Content-Type': 'application/json' },
            }),
        )
        vi.stubGlobal('fetch', connectorFetch)

        const response = await callPost({
            action: 'discover_folders',
            serviceAccountJson: '{"client_email":"service@example.com"}',
            principalEmail: 'admin@example.com',
            domain: 'example.com',
            params: {},
        })

        expect(response).toEqual({ status: 200, body: connectorResponse })
        expect(connectorFetch).toHaveBeenCalledOnce()
        const [url, init] = connectorFetch.mock.calls[0] as [string, RequestInit]
        expect(url).toBe('http://cm.test/action')
        const forwarded = JSON.parse(String(init.body)) as Record<string, unknown>
        expect(forwarded).not.toHaveProperty('source_id')
        expect(forwarded).toMatchObject({
            source_type: 'google_drive',
            user_id: 'admin-1',
            action: 'discover_folders',
            params: {},
            transient_credentials: {
                provider: 'google',
                auth_type: 'jwt',
                principal_email: 'admin@example.com',
                credentials: {
                    service_account_key: '{"client_email":"service@example.com"}',
                },
                config: { domain: 'example.com' },
            },
        })
    })

    it('rejects unknown actions and unknown top-level fields', async () => {
        expect((await callPost({ action: 'bogus', serviceAccountJson: '{}' })).status).toBe(400)
        expect(
            (await callPost({ action: 'discover_folders', serviceAccountJson: '{}', evil: 1 }))
                .status,
        ).toBe(400)
    })

    it('rejects unknown auth modes', async () => {
        const response = await callPost({
            action: 'discover_folders',
            serviceAccountJson: '{}',
            authMode: 'bogus_mode',
        })
        expect(response.status).toBe(400)
    })

    it('requires principal email + domain in DWD mode', async () => {
        const response = await callPost({
            action: 'discover_folders',
            serviceAccountJson: '{}',
        })
        expect(response.status).toBe(400)
    })

    it('forwards SA-direct discovery without principal/domain and with drive.readonly scope', async () => {
        const connectorResponse = { status: 'success', result: { items: [] } }
        const connectorFetch = vi.fn().mockResolvedValue(
            new Response(JSON.stringify(connectorResponse), {
                status: 200,
                headers: { 'Content-Type': 'application/json' },
            }),
        )
        vi.stubGlobal('fetch', connectorFetch)

        const response = await callPost({
            action: 'discover_folders',
            serviceAccountJson: '{"client_email":"sa@example.iam.gserviceaccount.com"}',
            authMode: 'service_account_direct',
            params: { auth_mode: 'service_account_direct' },
        })

        expect(response.status).toBe(200)
        const [url, init] = connectorFetch.mock.calls[0] as [string, RequestInit]
        expect(url).toBe('http://cm.test/action')
        const forwarded = JSON.parse(String(init.body)) as Record<string, unknown>
        expect(forwarded).toMatchObject({
            action: 'discover_folders',
            transient_credentials: {
                provider: 'google',
                auth_type: 'jwt',
                principal_email: null,
                credentials: {
                    service_account_key: '{"client_email":"sa@example.iam.gserviceaccount.com"}',
                },
                config: { scopes: ['https://www.googleapis.com/auth/drive.readonly'] },
            },
        })
    })

    it('forwards validate_shared_drive_access with drive ids in SA-direct mode', async () => {
        const connectorResponse = {
            status: 'success',
            result: {
                drives: [{ drive_id: 'd1', ok: true, role: 'organizer', error: null }],
            },
        }
        const connectorFetch = vi.fn().mockResolvedValue(
            new Response(JSON.stringify(connectorResponse), {
                status: 200,
                headers: { 'Content-Type': 'application/json' },
            }),
        )
        vi.stubGlobal('fetch', connectorFetch)

        const response = await callPost({
            action: 'validate_shared_drive_access',
            serviceAccountJson: '{"client_email":"sa@example.iam.gserviceaccount.com"}',
            authMode: 'service_account_direct',
            params: { drive_ids: ['d1', 'd2'] },
        })

        expect(response.status).toBe(200)
        const [url, init] = connectorFetch.mock.calls[0] as [string, RequestInit]
        const forwarded = JSON.parse(String(init.body)) as Record<string, unknown>
        expect(forwarded).toMatchObject({
            action: 'validate_shared_drive_access',
            params: { drive_ids: ['d1', 'd2'] },
        })
    })

    it('rejects validate_shared_drive_access without drive ids or outside SA-direct', async () => {
        expect(
            (
                await callPost({
                    action: 'validate_shared_drive_access',
                    serviceAccountJson: '{}',
                    authMode: 'service_account_direct',
                    params: { drive_ids: [] },
                })
            ).status,
        ).toBe(400)
        expect(
            (
                await callPost({
                    action: 'validate_shared_drive_access',
                    serviceAccountJson: '{}',
                    params: { drive_ids: ['d1'] },
                })
            ).status,
        ).toBe(400)
    })
})
