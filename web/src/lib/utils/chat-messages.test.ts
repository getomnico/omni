import { describe, expect, it } from 'vitest'
import { parseFinalizedAssistantMessage } from './chat-messages'

function finalizedAssistantMessage(content: unknown[], extras: Record<string, unknown> = {}) {
    return {
        id: 'message-1',
        chat_id: 'chat-1',
        parent_id: null,
        message_seq_num: 1,
        message: { role: 'assistant', content, ...extras },
        content_text: null,
        error: null,
        created_at: '2026-01-01T00:00:00+00:00',
    }
}

describe('parseFinalizedAssistantMessage', () => {
    it.each([
        ['null content block', [null]],
        ['malformed text block', [{ type: 'text', text: 42 }]],
        ['malformed tool-use block', [{ type: 'tool_use', id: 42, name: 'search', input: [] }]],
    ])('rejects a %s instead of accepting unknown content', (_description, content) => {
        expect(() => parseFinalizedAssistantMessage(finalizedAssistantMessage(content))).toThrow(
            'Invalid finalized assistant message',
        )
    })

    it('preserves signed thinking and provider-specific extras', () => {
        const message = {
            role: 'assistant',
            content: [
                {
                    type: 'thinking',
                    thinking: 'Reasoning text',
                    signature: 'signed-thinking-payload',
                },
                {
                    type: 'text',
                    text: 'A cited answer',
                    citations: [
                        {
                            type: 'search_result_location',
                            title: 'Citation source',
                            source: 'synthetic://citation-source',
                            cited_text: 'Citation source snippet',
                            start_block_index: 0,
                            end_block_index: 0,
                            search_result_index: 0,
                        },
                    ],
                },
            ],
            provider_metadata: { trace_id: 'trace-123' },
        }
        const parsed = parseFinalizedAssistantMessage(
            finalizedAssistantMessage(message.content, {
                provider_metadata: message.provider_metadata,
            }),
        )

        expect(parsed.message).toEqual(message)
    })
})
