import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'

type AiUpload = {
    id: string
    user_id: string
    filename: string
    content_type: string
    size_bytes: number
    created_at: string
}

export const GET: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('uploads')

    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const id = params.id
    if (!id) {
        return json({ error: 'id is required' }, { status: 400 })
    }

    let resp: Response
    try {
        resp = await fetch(`${env.AI_SERVICE_URL}/uploads/${id}`)
    } catch (err) {
        logger.error('Upload fetch failed', err as Error, {
            aiServiceUrl: env.AI_SERVICE_URL,
            uploadId: id,
        })
        return json({ error: 'Upload service unavailable' }, { status: 503 })
    }

    if (resp.status === 404) {
        return json({ error: 'Upload not found' }, { status: 404 })
    }
    if (!resp.ok) {
        return json({ error: 'Upstream error' }, { status: 502 })
    }

    const upload = (await resp.json()) as AiUpload
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
