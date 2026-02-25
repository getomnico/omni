import { env } from '$env/dynamic/private'
import { error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    const { chatId, path } = params
    const logger = locals.logger.child('artifacts')

    if (!chatId || !path) {
        throw error(400, 'Missing chatId or path')
    }

    // Auth check: verify chat exists and belongs to user
    const chat = await chatRepository.get(chatId)
    if (!chat) {
        throw error(404, 'Chat not found')
    }

    if (chat.userId !== locals.user?.id) {
        throw error(403, 'Forbidden')
    }

    try {
        const response = await fetch(`${env.AI_SERVICE_URL}/chat/${chatId}/artifacts/${path}`)

        if (!response.ok) {
            logger.warn('Artifact proxy failed', undefined, {
                chatId,
                path,
                status: response.status,
            })
            throw error(response.status, 'Artifact not found')
        }

        const contentType = response.headers.get('content-type') || 'application/octet-stream'
        const body = await response.arrayBuffer()

        return new Response(body, {
            headers: {
                'Content-Type': contentType,
                'Cache-Control': 'private, max-age=3600',
            },
        })
    } catch (err) {
        if ((err as any)?.status) throw err
        logger.error('Artifact proxy error', err, { chatId, path })
        throw error(502, 'Failed to fetch artifact')
    }
}
