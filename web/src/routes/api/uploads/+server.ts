import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'

export const POST: RequestHandler = async ({ request, locals }) => {
    const logger = locals.logger.child('uploads')

    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const incoming = await request.formData()
    const file = incoming.get('file')
    if (!(file instanceof File)) {
        return json({ error: 'file field is required' }, { status: 400 })
    }

    const forwarded = new FormData()
    forwarded.append('user_id', locals.user.id)
    forwarded.append('file', file, file.name)

    let resp: Response
    try {
        resp = await fetch(`${env.AI_SERVICE_URL}/uploads`, {
            method: 'POST',
            body: forwarded,
        })
    } catch (err) {
        logger.error('Upload proxy fetch failed', err as Error, {
            aiServiceUrl: env.AI_SERVICE_URL,
        })
        return json({ error: 'Upload service unavailable' }, { status: 503 })
    }

    const body = await resp.text()
    if (!resp.ok) {
        logger.warn('Upload proxy failed', undefined, { status: resp.status, body })
        return new Response(body, {
            status: resp.status,
            headers: { 'Content-Type': resp.headers.get('Content-Type') ?? 'application/json' },
        })
    }

    return new Response(body, {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
    })
}
