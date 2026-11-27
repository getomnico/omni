import { z } from 'zod'
import type { ChatMessage } from '$lib/server/db/schema'

const citationSchema = z.union([
    z
        .object({
            type: z.literal('char_location'),
            cited_text: z.string(),
            document_index: z.number(),
            document_title: z.string().nullable(),
            end_char_index: z.number(),
            start_char_index: z.number(),
        })
        .passthrough(),
    z
        .object({
            type: z.literal('page_location'),
            cited_text: z.string(),
            document_index: z.number(),
            document_title: z.string().nullable(),
            end_page_number: z.number(),
            start_page_number: z.number(),
        })
        .passthrough(),
    z
        .object({
            type: z.literal('content_block_location'),
            cited_text: z.string(),
            document_index: z.number(),
            document_title: z.string().nullable(),
            end_block_index: z.number(),
            start_block_index: z.number(),
        })
        .passthrough(),
    z
        .object({
            type: z.literal('search_result_location'),
            cited_text: z.string(),
            end_block_index: z.number(),
            search_result_index: z.number(),
            source: z.string(),
            start_block_index: z.number(),
            title: z.string().nullable(),
        })
        .passthrough(),
    z
        .object({
            type: z.literal('web_search_result_location'),
            cited_text: z.string(),
            encrypted_index: z.string(),
            title: z.string().nullable(),
            url: z.string(),
        })
        .passthrough(),
])

const assistantContentBlockSchema = z.union([
    z
        .object({
            type: z.literal('text'),
            text: z.string(),
            citations: z.array(citationSchema).nullable().optional(),
        })
        .passthrough(),
    z
        .object({
            type: z.literal('thinking'),
            thinking: z.string(),
            signature: z.string(),
        })
        .passthrough(),
    z.object({ type: z.literal('redacted_thinking'), data: z.string() }).passthrough(),
    z
        .object({
            type: z.literal('tool_use'),
            id: z.string(),
            name: z.string(),
            input: z.record(z.string(), z.json()),
        })
        .passthrough(),
])

const finalizedAssistantMessageSchema = z.object({
    id: z.string(),
    chat_id: z.string(),
    parent_id: z.string().nullable(),
    message_seq_num: z.number(),
    message: z
        .object({
            role: z.literal('assistant'),
            content: z.array(assistantContentBlockSchema),
        })
        .passthrough(),
    content_text: z.string().nullable(),
    error: z
        .object({
            message: z.string(),
            provider: z.string().nullable().optional(),
            model: z.string().nullable().optional(),
            statusCode: z.number().nullable().optional(),
        })
        .nullable(),
    created_at: z.iso.datetime({ offset: true }).transform((value) => new Date(value)),
})

export function parseFinalizedAssistantMessage(value: unknown): ChatMessage {
    const parsed = finalizedAssistantMessageSchema.safeParse(value)
    if (!parsed.success) throw new Error('Invalid finalized assistant message')

    const message = parsed.data
    return {
        id: message.id,
        chatId: message.chat_id,
        parentId: message.parent_id,
        message: message.message,
        contentText: message.content_text,
        error: message.error,
        messageSeqNum: message.message_seq_num,
        createdAt: message.created_at,
    }
}
