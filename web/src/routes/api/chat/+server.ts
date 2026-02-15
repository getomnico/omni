import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats'

export const POST: RequestHandler = async ({ request, locals }) => {
    const logger = locals.logger.child('chat')

    if (!locals.user?.id) {
        logger.warn('Attempted to create chat without valid user')
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const userId = locals.user.id

    let modelId: string | undefined
    try {
        const body = await request.json()
        modelId = body.modelId || undefined
    } catch {
        // No body or invalid JSON is fine — modelId stays undefined
    }

    logger.debug('Creating new chat', { userId, modelId })

    try {
        const chat = await chatRepository.create(userId, undefined, modelId)

        logger.info('Chat created successfully', {
            userId,
            chatId: chat.id,
        })

        return json({ chatId: chat.id }, { status: 200 })
    } catch (error) {
        logger.error('Error creating chat', error)
        return json(
            {
                error: 'Failed to create chat',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
