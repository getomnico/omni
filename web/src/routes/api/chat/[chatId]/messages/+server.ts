import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository, chatMessageRepository } from '$lib/server/db/chats'
import { getAgent } from '$lib/server/db/agents.js'
import type { OmniUploadBlock, OmniMentionBlock } from '$lib/types/message'
import type {
    MessageParam,
    TextBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
} from '@anthropic-ai/sdk/resources/messages'
import { getChatStreamStatus } from '$lib/server/ai-stream-status.js'
import { z } from 'zod'
import { isValid } from 'ulid'

const ULID_REGEX = /^[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{26}$/i

function isValidUlid(s: string): boolean {
    return ULID_REGEX.test(s) && isValid(s)
}

const MAX_CONTENT_LENGTH = 100_000

const mentionedDocumentSchema = z.object({
    document_id: z.string().min(1).max(100).refine(isValidUlid, 'Invalid ULID'),
    title: z.string().min(1).max(500),
})

const ulidString = z.string().min(26).max(26).refine(isValidUlid, 'Invalid ULID')

const messageRequestSchema = z.object({
    content: z.string().max(MAX_CONTENT_LENGTH),
    parentId: z.string().min(1).optional(),
    attachmentIds: z.array(ulidString).max(50).optional().default([]),
    mentionedDocuments: z.array(mentionedDocumentSchema).max(25).optional().default([]),
})

type UserMessageBlock = OmniUploadBlock | OmniMentionBlock | TextBlockParam

async function chatOwnerGuard(
    chatId: string,
    userId: string,
    userRole: string,
): Promise<{ ok: true; chat: { agentId?: string | null } } | { ok: false; response: Response }> {
    const chat = await chatRepository.get(chatId)
    if (!chat) {
        return { ok: false, response: json({ error: 'Chat not found' }, { status: 404 }) }
    }
    if (chat.userId !== userId) {
        return { ok: false, response: json({ error: 'Forbidden' }, { status: 403 }) }
    }
    if (chat.agentId) {
        const agent = await getAgent(chat.agentId)
        if (!agent) {
            return { ok: false, response: json({ error: 'Chat agent not found' }, { status: 404 }) }
        }
        if (agent.agentType === 'org' && userRole !== 'admin') {
            return { ok: false, response: json({ error: 'Forbidden' }, { status: 403 }) }
        }
        if (agent.agentType === 'user' && agent.userId !== userId) {
            return { ok: false, response: json({ error: 'Forbidden' }, { status: 403 }) }
        }
    }
    return { ok: true, chat }
}

function interruptedToolResultMessage(message: MessageParam): MessageParam | null {
    if (message.role !== 'assistant' || !Array.isArray(message.content)) return null
    const toolUses = message.content.filter(
        (block): block is ToolUseBlockParam => block.type === 'tool_use',
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
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }
    const guard = await chatOwnerGuard(chatId, locals.user.id, locals.user.role)
    if (!guard.ok) return guard.response
    const chatMessages = await chatMessageRepository.getByChatId(chatId)
    const messages = chatMessages.map((msg) => ({
        id: msg.id,
        chat_id: msg.chatId,
        parent_id: msg.parentId,
        message_seq_num: msg.messageSeqNum,
        message: msg.message,
        created_at: msg.createdAt,
    }))
    return json(messages, { status: 200 })
}

export const POST: RequestHandler = async ({ params, request, locals, fetch }) => {
    const logger = locals.logger.child('chat')
    const chatId = params.chatId
    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
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

    const parsed = messageRequestSchema.safeParse(rawBody)
    if (!parsed.success) {
        const details = parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`)
        return json({ error: 'Invalid request', details }, { status: 400 })
    }

    const trimmedText = parsed.data.content.trim()
    const attachmentIds = parsed.data.attachmentIds
    const mentionedDocuments = parsed.data.mentionedDocuments

    // Pure whitespace with no rich blocks → 400
    if (trimmedText === '' && attachmentIds.length === 0 && mentionedDocuments.length === 0) {
        return json({ error: 'Content or attachments are required' }, { status: 400 })
    }

    logger.debug('Adding message to chat', { chatId, userId: locals.user.id })

    if (attachmentIds.length > 0) {
        const ownershipResult = await verifyAttachmentOwnership(attachmentIds, fetch)
        if (!ownershipResult.ok) return ownershipResult.response
    }

    const guard = await chatOwnerGuard(chatId, locals.user.id, locals.user.role)
    if (!guard.ok) return guard.response

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
    } catch {
        logger.warn('Could not check stream status before adding message', { chatId })
    }

    let userMessage: { role: 'user'; content: string | UserMessageBlock[] }
    const mentionBlocks: UserMessageBlock[] = mentionedDocuments.map((doc) => ({
        type: 'document',
        source: {
            type: 'omni_mention',
            document_id: doc.document_id,
            title: doc.title,
            ...(doc.source_type ? { source_type: doc.source_type } : {}),
            ...(doc.content_type ? { content_type: doc.content_type } : {}),
        },
    }))
    const uploadBlocks: OmniUploadBlock[] = attachmentIds.map((id) => ({
        type: 'document',
        source: { type: 'omni_upload', upload_id: id },
    }))
    const hasRichBlocks = mentionBlocks.length > 0 || uploadBlocks.length > 0
    if (hasRichBlocks) {
        const blocks: UserMessageBlock[] = [...mentionBlocks, ...uploadBlocks]
        if (trimmedText !== '') {
            blocks.push({ type: 'text', text: trimmedText })
        }
        userMessage = { role: 'user', content: blocks }
    } else {
        userMessage = { role: 'user', content: trimmedText }
    }

    let parentId = parsed.data.parentId?.trim() || undefined
    let parentMessage = parentId
        ? await chatMessageRepository.getByIdInChat(chatId, parentId)
        : null
    if (parentId && !parentMessage) {
        logger.warn('Ignoring unknown client-provided parent message id', { chatId, parentId })
        parentId = undefined
        parentMessage = null
    }
    if (!parentMessage) {
        parentMessage = await chatMessageRepository.getLastMessageInActivePath(chatId)
        parentId = parentMessage?.id
    }
    if (parentMessage) {
        const repairMessage = interruptedToolResultMessage(parentMessage.message)
        if (repairMessage) {
            const savedRepairMessage = await chatMessageRepository.create(
                chatId,
                repairMessage,
                parentMessage.id,
            )
            parentId = savedRepairMessage.id
            logger.warn('Inserted failed tool_result for interrupted tool call', {
                chatId,
                repairMessageId: savedRepairMessage.id,
            })
        }
    }

    const savedMessage = await chatMessageRepository.create(
        chatId,
        userMessage as unknown as MessageParam,
        parentId,
    )

    return json(
        {
            messageId: savedMessage.id,
            status: 'created',
        },
        { status: 200 },
    )
}

async function verifyAttachmentOwnership(
    attachmentIds: string[],
    fetchFn: typeof globalThis.fetch,
): Promise<{ ok: true } | { ok: false; response: Response }> {
    for (const id of attachmentIds) {
        try {
            const resp = await fetchFn(`/api/uploads/${id}`)
            if (resp.status === 404) {
                return {
                    ok: false,
                    response: json({ error: `Attachment ${id}: not found` }, { status: 404 }),
                }
            }
            if (resp.status >= 500) {
                return {
                    ok: false,
                    response: json(
                        { error: `Attachment ${id}: upload service unavailable` },
                        { status: 502 },
                    ),
                }
            }
            if (!resp.ok) {
                return {
                    ok: false,
                    response: json({ error: `Attachment ${id}: access denied` }, { status: 403 }),
                }
            }
        } catch {
            return {
                ok: false,
                response: json(
                    { error: `Attachment ${id}: upload service unavailable` },
                    { status: 502 },
                ),
            }
        }
    }
    return { ok: true }
}
