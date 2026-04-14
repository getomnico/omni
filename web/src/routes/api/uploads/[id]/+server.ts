import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const id = params.id
    if (!id) {
        return json({ error: 'id is required' }, { status: 400 })
    }

    const resp = await fetch(`${env.AI_SERVICE_URL}/uploads/${id}`)
    if (resp.status === 404) {
        return json({ error: 'Upload not found' }, { status: 404 })
    }
    if (!resp.ok) {
        return json({ error: 'Upstream error' }, { status: 502 })
    }

    const upload = await resp.json()
    if (upload.user_id !== locals.user.id) {
        return json({ error: 'Not found' }, { status: 404 })
    }

    return json({
        id: upload.id,
        filename: upload.filename,
        contentType: upload.content_type,
        sizeBytes: upload.size_bytes,
        createdAt: upload.created_at,
    })
}
