import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { fetchRemoteImageBytes, UrlFetchError } from '$lib/server/uploads/remoteImage'

// Image-fetch proxy for pastes that only carry text/html (Safari web-image
// pastes). Returns the raw bytes; the client wraps them in a File and uploads
// through the normal /api/uploads flow, so storage stays in one place.
export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let url: unknown
    try {
        url = (await request.json())?.url
    } catch {
        error(400, 'Invalid JSON body')
    }
    if (typeof url !== 'string' || url.length === 0 || url.length > 2048) {
        return json({ error: 'url field is required' }, { status: 400 })
    }

    try {
        const { bytes, contentType } = await fetchRemoteImageBytes(url)
        return new Response(bytes, {
            status: 200,
            headers: {
                'Content-Type': 'application/octet-stream',
                'X-Image-Content-Type': contentType,
                'Cache-Control': 'no-store',
            },
        })
    } catch (err) {
        if (err instanceof UrlFetchError) {
            return json({ error: err.message }, { status: err.status })
        }
        return json({ error: 'Failed to fetch pasted image' }, { status: 502 })
    }
}
