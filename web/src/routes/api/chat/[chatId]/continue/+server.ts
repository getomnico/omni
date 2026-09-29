import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatMessageRepository, chatRepository } from '$lib/server/db/chats.js'
import { getAgent } from '$lib/server/db/agents.js'

export const POST: RequestHandler = async ({ params, locals, request }) => {
    const chatId = params.chatId
    if (!locals.user?.id) return json({ error: 'User not authenticated' }, { status: 401 })

    const chat = await chatRepository.get(chatId)
    if (!chat) return json({ error: 'Chat not found' }, { status: 404 })
    if (chat.userId !== locals.user.id) return json({ error: 'Forbidden' }, { status: 403 })
    if (chat.agentId) {
        const agent = await getAgent(chat.agentId)
        if (!agent) return json({ error: 'Chat agent not found' }, { status: 404 })
        if (agent.agentType === 'org' && locals.user.role !== 'admin') {
            return json({ error: 'Forbidden' }, { status: 403 })
        }
        if (agent.agentType === 'user' && agent.userId !== locals.user.id) {
            return json({ error: 'Forbidden' }, { status: 403 })
        }
    }

    let payload: unknown
    try {
        payload = await request.json()
    } catch {
        return json({ error: 'Invalid JSON request body' }, { status: 400 })
    }
    if (
        typeof payload !== 'object' ||
        payload === null ||
        !('terminalMessageId' in payload) ||
        typeof payload.terminalMessageId !== 'string'
    ) {
        return json({ error: 'terminalMessageId is required' }, { status: 400 })
    }

    const continuedMessage = await chatMessageRepository.continueLimitedTurn(
        chatId,
        payload.terminalMessageId,
    )
    if (!continuedMessage) {
        return json({ error: 'This turn can no longer be continued' }, { status: 409 })
    }
    return json({ messageId: continuedMessage.id }, { status: 200 })
}
