import { describe, expect, it } from 'vitest'
import { parseChatMessagesResponse } from './chat-messages'

function persistedAssistantMessage(content: unknown[], extras: Record<string, unknown> = {}) {
    return {
        id: 'message-1',
        chat_id: 'chat-1',
        parent_id: null,
        message_seq_num: 1,
        message: { role: 'assistant', content, ...extras },
        content_text: null,
        error: null,
        created_at: '2026-01-01T00:00:00.000Z',
    }
}

describe('parseChatMessagesResponse', () => {
    it.each([
        ['null content block', [null]],
        ['malformed text block', [{ type: 'text', text: 42 }]],
        [
            'malformed tool-use block',
            [{ type: 'tool_use', id: 42, name: 'search', input: [] }],
        ],
    ])('rejects a %s instead of accepting unknown content', (_description, content) => {
        expect(() =>
            parseChatMessagesResponse([persistedAssistantMessage(content)]),
        ).toThrow('Invalid chat messages response')
    })

    it('preserves valid thinking signatures and provider-specific message extras', () => {
        const message = {
            role: 'assistant',
            content: [
                {
                    type: 'thinking',
                    thinking: 'Reasoning text',
                    signature: 'signed-thinking-payload',
                },
            ],
            provider_metadata: { trace_id: 'trace-123' },
        }
        const [parsed] = parseChatMessagesResponse([
            {
                ...persistedAssistantMessage(message.content, {
                    provider_metadata: message.provider_metadata,
                }),
            },
        ])

        expect(parsed.message).toEqual(message)
    })
})
