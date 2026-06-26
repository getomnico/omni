import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'

export const POST: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat-stop')

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

    // Validate user owns the chat
    if (chat.userId !== locals.user.id) {
        return json({ error: 'Forbidden' }, { status: 403 })
    }

    const replayPath = env.OMNI_CHAT_STREAM_REPLAY_PATH?.trim()
    if (replayPath) {
        return json({ status: 'ok' })
    }

    try {
        const response = await fetch(`${env.AI_SERVICE_URL}/chat/${chatId}/cancel`, {
            method: 'POST',
        })
        if (!response.ok) {
            logger.warn('AI service cancel returned non-OK', {
                chatId,
                status: response.status,
            })
        }
    } catch (err) {
        logger.error('Failed to cancel chat stream', err, { chatId })
    }

    // Best-effort: report success even if the AI service is unreachable.
    return json({ status: 'ok' })
}
