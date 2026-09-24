import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import { describe, expect, it, vi } from 'vitest'

const serviceWorkerSource = readFileSync(new URL('./service-worker.js', import.meta.url), 'utf8')

type FetchEventHandler = (event: {
    request: Request
    preloadResponse: Promise<Response | undefined>
    respondWith: (response: Promise<Response>) => void
}) => void

function createServiceWorker(fetchImpl: typeof fetch) {
    const listeners = new Map<string, (...args: never[]) => void>()
    const self = {
        registration: { scope: 'https://omni.test/' },
        addEventListener: (type: string, listener: (...args: never[]) => void) =>
            listeners.set(type, listener),
    }
    runInNewContext(serviceWorkerSource, {
        self,
        URL,
        Response,
        fetch: fetchImpl,
        caches: { open: async () => ({ match: async () => undefined }) },
        AbortController,
        setTimeout: (callback: () => void) => globalThis.setTimeout(callback, 5),
        clearTimeout: (timer: ReturnType<typeof setTimeout>) => globalThis.clearTimeout(timer),
    })
    return listeners.get('fetch') as unknown as FetchEventHandler
}

async function navigate(
    handler: FetchEventHandler,
    preloadResponse: Promise<Response | undefined> = Promise.resolve(undefined),
): Promise<Response> {
    let response: Promise<Response> | undefined
    handler({
        request: { url: 'https://omni.test/integrations', mode: 'navigate' } as Request,
        preloadResponse,
        respondWith: (value) => (response = value),
    })
    if (!response) throw new Error('Service worker did not handle navigation')
    return response
}

describe('PWA navigation handling', () => {
    it('passes through a slow successful navigation without a timeout', async () => {
        const fetchImpl = vi.fn(
            () =>
                new Promise<Response>((resolve) =>
                    setTimeout(() => resolve(new Response('ok')), 40),
                ),
        ) as unknown as typeof fetch
        const handler = createServiceWorker(fetchImpl)

        const response = await navigate(handler)

        expect(response.status).toBe(200)
        expect(await response.text()).toBe('ok')
        expect(fetchImpl).toHaveBeenCalledTimes(1)
    })

    it('passes through real server errors', async () => {
        const handler = createServiceWorker(
            async () => new Response('server error', { status: 503 }),
        )

        const response = await navigate(handler)

        expect(response.status).toBe(503)
        expect(await response.text()).toBe('server error')
    })

    it('uses a successful navigation preload without issuing a duplicate fetch', async () => {
        const fetchImpl = vi.fn() as unknown as typeof fetch
        const handler = createServiceWorker(fetchImpl)
        const response = await navigate(handler, Promise.resolve(new Response('preloaded')))

        expect(await response.text()).toBe('preloaded')
        expect(fetchImpl).not.toHaveBeenCalled()
    })

    it('falls back to one direct request when navigation preload rejects', async () => {
        const fetchImpl = vi.fn(async () => new Response('retried')) as unknown as typeof fetch
        const handler = createServiceWorker(fetchImpl)

        const failedPreload = Promise.resolve().then(() => {
            throw new TypeError('preload failed')
        })
        const response = await navigate(handler, failedPreload)

        expect(await response.text()).toBe('retried')
        expect(fetchImpl).toHaveBeenCalledTimes(1)
    })

    it('uses the recovery document only when the network request fails', async () => {
        const handler = createServiceWorker(async () => {
            throw new TypeError('network unavailable')
        })

        const response = await navigate(handler)

        expect(response.status).toBe(503)
        expect(await response.text()).toContain("Omni couldn't connect")
    })
})
