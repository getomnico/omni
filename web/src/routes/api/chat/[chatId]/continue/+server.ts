import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import {
    chatMessageRepository,
    chatRepository,
    continuationMessageId,
} from '$lib/server/db/chats.js'
import { getAgent } from '$lib/server/db/agents.js'
import { getChatStreamStatus } from '$lib/server/ai-stream-status.js'

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

    const terminalMessageId = payload.terminalMessageId
    const continuationId = continuationMessageId(terminalMessageId)
    const activePath = await chatMessageRepository.getActivePath(chatId)
    const existingContinuation = activePath.find((message) => message.id === continuationId)
    if (existingContinuation) {
        return json({ messageId: existingContinuation.id }, { status: 200 })
    }
    if (activePath.at(-1)?.id !== terminalMessageId) {
        return json({ error: 'This turn can no longer be continued' }, { status: 409 })
    }

    try {
        const status = await getChatStreamStatus(chatId)
        if (!status.iterationLimitReached || status.iterationLimitMessageId !== terminalMessageId) {
            return json({ error: 'This turn can no longer be continued' }, { status: 409 })
        }
    } catch (err) {
        locals.logger.error('Failed to verify iteration limit before continuation', err, {
            chatId,
        })
        return json({ error: 'Unable to verify this turn for continuation' }, { status: 502 })
    }

    const continuedMessage = await chatMessageRepository.continueLimitedTurn(
        chatId,
        terminalMessageId,
        continuationId,
    )
    if (!continuedMessage) {
        return json({ error: 'This turn can no longer be continued' }, { status: 409 })
    }
    return json({ messageId: continuedMessage.id }, { status: 200 })
}
