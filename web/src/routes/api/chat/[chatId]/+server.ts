import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats'

export const GET: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat')

    const chatId = params.chatId
    if (!chatId) {
        logger.warn('Missing chatId parameter in chat details request')
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    logger.debug('Fetching chat details')

    try {
        const chat = await chatRepository.get(chatId)

        if (!chat) {
            logger.warn('Chat not found')
            return json({ error: 'Chat not found' }, { status: 404 })
        }

        logger.info('Chat details retrieved successfully')

        // Convert to match AI service response format
        const chatDetails = {
            id: chat.id,
            user_id: chat.userId,
            title: chat.title,
            created_at: chat.createdAt,
            updated_at: chat.updatedAt,
        }

        return json(chatDetails, { status: 200 })
    } catch (error) {
        logger.error('Error fetching chat details', error)
        return json(
            {
                error: 'Failed to fetch chat details',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}

export const PATCH: RequestHandler = async ({ params, locals, request }) => {
    const logger = locals.logger.child('chat')
    const chatId = params.chatId

    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    const chat = await chatRepository.get(chatId)
    if (!chat) {
        return json({ error: 'Chat not found' }, { status: 404 })
    }
    if (chat.userId !== locals.user.id) {
        return json({ error: 'Forbidden' }, { status: 403 })
    }

    try {
        const body = await request.json()
        let updatedChat = chat

        if (typeof body.title === 'string') {
            const result = await chatRepository.updateTitle(chatId, body.title)
            if (result) updatedChat = result
        }

        if (typeof body.isStarred === 'boolean') {
            const result = await chatRepository.toggleStar(chatId, body.isStarred)
            if (result) updatedChat = result
        }

        logger.info('Chat updated')
        return json(updatedChat)
    } catch (error) {
        logger.error('Error updating chat', error)
        return json(
            {
                error: 'Failed to update chat',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}

export const DELETE: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat')
    const chatId = params.chatId

    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    const chat = await chatRepository.get(chatId)
    if (!chat) {
        return json({ error: 'Chat not found' }, { status: 404 })
    }
    if (chat.userId !== locals.user.id) {
        return json({ error: 'Forbidden' }, { status: 403 })
    }

    try {
        await chatRepository.delete(chatId)
        logger.info('Chat deleted')
        return new Response(null, { status: 204 })
    } catch (error) {
        logger.error('Error deleting chat', error)
        return json(
            {
                error: 'Failed to delete chat',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
