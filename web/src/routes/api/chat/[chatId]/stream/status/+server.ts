import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'
import { getChatStreamStatus } from '$lib/server/ai-stream-status.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat')
    const chatId = params.chatId
    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const chat = await chatRepository.get(chatId)
    if (!chat) {
        return json({ error: 'Chat not found' }, { status: 404 })
    }
    if (chat.userId !== locals.user.id) {
        return json({ error: 'Forbidden' }, { status: 403 })
    }

    try {
        return json(await getChatStreamStatus(chatId))
    } catch (error) {
        logger.warn('Failed to fetch chat stream status', error, { chatId })
        return json({ error: 'Failed to fetch stream status' }, { status: 502 })
    }
}
