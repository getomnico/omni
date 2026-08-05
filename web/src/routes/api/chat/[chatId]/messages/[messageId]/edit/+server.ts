import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository, chatMessageRepository } from '$lib/server/db/chats'
import { getAgent } from '$lib/server/db/agents.js'
import { getChatStreamStatus } from '$lib/server/ai-stream-status.js'
import type { OmniMentionBlock, OmniUploadBlock } from '$lib/types/message'
import type { MessageParam, TextBlockParam } from '@anthropic-ai/sdk/resources/messages'
import { z } from 'zod'

const editRequestSchema = z
    .object({
        content: z.string().max(100_000),
    })
    .strict()

type UserMessageBlock = OmniMentionBlock | OmniUploadBlock | TextBlockParam

type UnknownSource = {
    type?: unknown
    upload_id?: unknown
    document_id?: unknown
    title?: unknown
    source_type?: unknown
    content_type?: unknown
}

type UnknownBlock = {
    type?: unknown
    source?: UnknownSource
}

function parseStoredRichBlock(block: unknown): OmniMentionBlock | OmniUploadBlock | null {
    if (!block || typeof block !== 'object') return null

    const candidate = block as UnknownBlock
    const source = candidate.source
    if (!source || typeof source !== 'object') return null

    if (
        candidate.type === 'document' &&
        source.type === 'omni_mention' &&
        typeof source.document_id === 'string' &&
        typeof source.title === 'string' &&
        (source.source_type === undefined || typeof source.source_type === 'string') &&
        (source.content_type === undefined || typeof source.content_type === 'string')
    ) {
        return {
            type: 'document',
            source: {
                type: 'omni_mention',
                document_id: source.document_id,
                title: source.title,
                ...(source.source_type ? { source_type: source.source_type } : {}),
                ...(source.content_type ? { content_type: source.content_type } : {}),
            },
        }
    }

    if (
        (candidate.type === 'document' || candidate.type === 'image') &&
        source.type === 'omni_upload' &&
        typeof source.upload_id === 'string'
    ) {
        return {
            type: candidate.type,
            source: { type: 'omni_upload', upload_id: source.upload_id },
        }
    }

    return null
}

export const POST: RequestHandler = async ({ params, request, locals }) => {
    const logger = locals.logger.child('chat')
    const { chatId, messageId } = params

    if (!chatId || !messageId) {
        return json({ error: 'chatId and messageId are required' }, { status: 400 })
    }
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let rawBody: unknown
    try {
        rawBody = await request.json()
    } catch {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const parsed = editRequestSchema.safeParse(rawBody)
    if (!parsed.success) {
        const details = parsed.error.issues.map(
            (issue) => `${issue.path.join('.')}: ${issue.message}`,
        )
        return json({ error: 'Invalid request', details }, { status: 400 })
    }

    const trimmedText = parsed.data.content.trim()

    try {
        const chat = await chatRepository.get(chatId)
        if (!chat) {
            return json({ error: 'Chat not found' }, { status: 404 })
        }
        if (chat.userId !== locals.user.id) {
            return json({ error: 'Forbidden' }, { status: 403 })
        }
        if (chat.agentId) {
            const agent = await getAgent(chat.agentId)
            if (!agent) {
                return json({ error: 'Chat agent not found' }, { status: 404 })
            }
            if (agent.agentType === 'org' && locals.user.role !== 'admin') {
                return json({ error: 'Forbidden' }, { status: 403 })
            }
            if (agent.agentType === 'user' && agent.userId !== locals.user.id) {
                return json({ error: 'Forbidden' }, { status: 403 })
            }
        }

        try {
            const streamStatus = await getChatStreamStatus(chatId)
            if (streamStatus.running) {
                return json(
                    {
                        error: 'A response is still in progress for this chat. Reconnect to the stream before editing a message.',
                        streamActive: true,
                    },
                    { status: 409 },
                )
            }
        } catch (error) {
            logger.warn('Could not check stream status before editing message', {
                chatId,
                messageId,
                error,
            })
        }

        const originalMessage = await chatMessageRepository.getByIdInChat(chatId, messageId)
        if (!originalMessage) {
            return json({ error: 'Message not found' }, { status: 404 })
        }
        if (originalMessage.message.role !== 'user') {
            return json({ error: 'Only user messages can be edited' }, { status: 400 })
        }

        const richBlocks = Array.isArray(originalMessage.message.content)
            ? originalMessage.message.content
                  .map(parseStoredRichBlock)
                  .filter((block): block is OmniMentionBlock | OmniUploadBlock => block !== null)
            : []

        if (!trimmedText && richBlocks.length === 0) {
            return json({ error: 'Content or rich blocks are required' }, { status: 400 })
        }

        const blocks: UserMessageBlock[] = [...richBlocks]
        if (trimmedText) {
            blocks.push({ type: 'text', text: trimmedText })
        }

        const userMessage = { role: 'user' as const, content: blocks }
        const savedMessage = await chatMessageRepository.create(
            chatId,
            userMessage as unknown as MessageParam,
            originalMessage.parentId ?? undefined,
        )

        logger.info('Message edited (new branch created)', {
            chatId,
            originalMessageId: messageId,
            newMessageId: savedMessage.id,
        })

        return json(
            {
                messageId: savedMessage.id,
                message: userMessage,
                status: 'created',
            },
            { status: 200 },
        )
    } catch (error) {
        logger.error('Error editing message', { chatId, messageId, error })
        return json(
            {
                error: 'Failed to edit message',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
