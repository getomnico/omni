import type { MessageParam } from '@anthropic-ai/sdk/resources/messages'
import { z } from 'zod'
import type { ChatMessage } from '$lib/server/db/schema'

const streamErrorSchema = z.object({
    message: z.string(),
    provider: z.string().nullable().optional(),
    model: z.string().nullable().optional(),
    statusCode: z.number().nullable().optional(),
})

const persistedChatMessageSchema = z.object({
    id: z.string(),
    chat_id: z.string(),
    parent_id: z.string().nullable(),
    message_seq_num: z.number(),
    message: z
        .object({
            role: z.enum(['assistant', 'user']),
            content: z.union([z.string(), z.array(z.unknown())]),
        })
        .passthrough(),
    content_text: z.string().nullable(),
    error: streamErrorSchema.nullable(),
    created_at: z.coerce.date(),
})

const chatMessagesResponseSchema = z.array(persistedChatMessageSchema)

export function parseChatMessagesResponse(value: unknown): ChatMessage[] {
    const parsed = chatMessagesResponseSchema.safeParse(value)
    if (!parsed.success) throw new Error('Invalid chat messages response')

    return parsed.data.map((message) => ({
        id: message.id,
        chatId: message.chat_id,
        parentId: message.parent_id,
        message: message.message as MessageParam,
        contentText: message.content_text,
        error: message.error,
        messageSeqNum: message.message_seq_num,
        createdAt: message.created_at,
    }))
}
