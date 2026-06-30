import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository, chatMessageRepository } from '$lib/server/db/chats'
import { getAgent } from '$lib/server/db/agents.js'
import type { OmniUploadBlock } from '$lib/types/message'
import type {
    MessageParam,
    TextBlockParam,
    ToolResultBlockParam,
} from '@anthropic-ai/sdk/resources/messages'
import { getChatStreamStatus } from '$lib/server/ai-stream-status.js'

interface MessageRequest {
    content: string
    parentId?: string
    attachmentIds?: string[]
}

type UserMessageBlock = OmniUploadBlock | TextBlockParam

type AssistantToolUseBlock = {
    type: 'tool_use'
    id: string
    name: string
}

function interruptedToolResultMessage(message: unknown): MessageParam | null {
    if (!message || typeof message !== 'object') return null
    const candidate = message as Record<string, unknown>
    if (candidate.role !== 'assistant' || !Array.isArray(candidate.content)) return null

    const toolUses = candidate.content.filter(
        (block): block is AssistantToolUseBlock =>
            !!block &&
            typeof block === 'object' &&
            (block as Record<string, unknown>).type === 'tool_use' &&
            typeof (block as Record<string, unknown>).id === 'string' &&
            typeof (block as Record<string, unknown>).name === 'string',
    )
    if (toolUses.length === 0) return null

    const content: ToolResultBlockParam[] = toolUses.map((toolUse) => ({
        type: 'tool_result',
        tool_use_id: toolUse.id,
        content: [
            {
                type: 'text',
                text: `Tool call ${toolUse.name} did not complete because the previous response was interrupted. Treat this tool call as failed and retry it if the result is still needed.`,
            },
        ],
        is_error: true,
    }))

    return { role: 'user', content }
}

export const GET: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat')

    const chatId = params.chatId
    if (!chatId) {
        logger.warn('Missing chatId parameter in messages request')
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    logger.debug('Fetching chat messages', { chatId })

    try {
        // First check if chat exists
        const chat = await chatRepository.get(chatId)
        if (!chat) {
            logger.warn('Chat not found', { chatId })
            return json({ error: 'Chat not found' }, { status: 404 })
        }

        // Agent chats require admin access
        if (chat.agentId) {
            const agent = await getAgent(chat.agentId)
            if (agent?.agentType === 'org' && locals.user?.role !== 'admin') {
                throw error(403, 'Admin access required for agent chats')
            }
        }

        // Get messages for the chat
        const chatMessages = await chatMessageRepository.getByChatId(chatId)

        logger.info('Chat messages retrieved successfully', {
            chatId,
            messageCount: chatMessages.length,
        })

        // Convert to match AI service response format
        const messages = chatMessages.map((msg) => ({
            id: msg.id,
            chat_id: msg.chatId,
            parent_id: msg.parentId,
            message_seq_num: msg.messageSeqNum,
            message: msg.message,
            created_at: msg.createdAt,
        }))

        return json(messages, { status: 200 })
    } catch (error) {
        logger.error('Error fetching chat messages', error, { chatId })
        return json(
            {
                error: 'Failed to fetch messages',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}

export const POST: RequestHandler = async ({ params, request, locals }) => {
    const logger = locals.logger.child('chat')

    const chatId = params.chatId
    if (!chatId) {
        logger.warn('Missing chatId parameter in message request')
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    if (!locals.user?.id) {
        logger.warn('Attempted to post message without valid user')
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let messageRequest: MessageRequest
    try {
        messageRequest = await request.json()
    } catch (error) {
        logger.warn('Invalid JSON in message request', error)
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const trimmedText = messageRequest.content?.trim() ?? ''
    const attachmentIds = messageRequest.attachmentIds ?? []
    if (trimmedText === '' && attachmentIds.length === 0) {
        logger.warn('Empty content in message request')
        return json({ error: 'Content or attachments are required' }, { status: 400 })
    }

    logger.debug('Adding message to chat', {
        chatId,
        content: messageRequest.content.substring(0, 100),
        userId: locals.user.id,
    })

    try {
        // First check if chat exists
        const chat = await chatRepository.get(chatId)
        if (!chat) {
            logger.warn('Chat not found', { chatId })
            return json({ error: 'Chat not found' }, { status: 404 })
        }

        // Agent chats require admin access
        if (chat.agentId) {
            const agent = await getAgent(chat.agentId)
            if (agent?.agentType === 'org' && locals.user?.role !== 'admin') {
                throw error(403, 'Admin access required for agent chats')
            }
        }

        try {
            const streamStatus = await getChatStreamStatus(chatId)
            if (streamStatus.running) {
                return json(
                    {
                        error: 'A response is still in progress for this chat. Reconnect to the stream before sending another message.',
                        streamActive: true,
                    },
                    { status: 409 },
                )
            }
        } catch (error) {
            logger.warn('Could not check stream status before adding message', error, { chatId })
        }

        // Create the user message in MessageParam format. If there are attachments, build
        // the content as an array of blocks: omni_upload document blocks first, then text.
        let userMessage: { role: 'user'; content: string | UserMessageBlock[] }
        if (attachmentIds.length > 0) {
            const uploadBlocks: UploadBlock[] = attachmentIds.map((id) => ({
                type: 'document',
                source: { type: 'omni_upload', upload_id: id },
            }))
            const blocks: UserMessageBlock[] = [...uploadBlocks]
            if (trimmedText !== '') {
                blocks.push({ type: 'text', text: trimmedText })
            }
            userMessage = { role: 'user', content: blocks }
        } else {
            userMessage = { role: 'user', content: trimmedText }
        }

        // Determine parentId: use provided value, or find the last message in the active path.
        // If the active leaf is an assistant tool_use without a tool_result, first insert an
        // error tool_result so the next user turn does not create invalid provider history.
        let parentId = messageRequest.parentId
        if (!parentId) {
            const lastMessage = await chatMessageRepository.getLastMessageInActivePath(chatId)
            if (lastMessage) {
                const repairMessage = interruptedToolResultMessage(lastMessage.message)
                if (repairMessage) {
                    const savedRepairMessage = await chatMessageRepository.create(
                        chatId,
                        repairMessage,
                        lastMessage.id,
                    )
                    parentId = savedRepairMessage.id
                    logger.warn('Inserted failed tool_result for interrupted tool call', {
                        chatId,
                        repairMessageId: savedRepairMessage.id,
                    })
                } else {
                    parentId = lastMessage.id
                }
            }
        }

        // Save message to database
        const savedMessage = await chatMessageRepository.create(chatId, userMessage, parentId)

        logger.info('Message added successfully', {
            chatId,
            messageId: savedMessage.id,
            userId: locals.user.id,
        })

        return json(
            {
                messageId: savedMessage.id,
                status: 'created',
            },
            { status: 200 },
        )
    } catch (error) {
        logger.error('Error adding message', error, { chatId })
        return json(
            {
                error: 'Failed to add message',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
