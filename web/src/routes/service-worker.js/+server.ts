import swSource from '$lib/pwa/service-worker.js?raw'
import type { RequestHandler } from './$types.js'

// Served with no-cache so browser update checks pick up new service workers
// promptly instead of a heuristic-cached copy (up to ~24h stale).
export const GET: RequestHandler = async () =>
    new Response(swSource, {
        headers: {
            'Content-Type': 'text/javascript; charset=utf-8',
            'Cache-Control': 'no-cache',
        },
    })
