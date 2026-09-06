import { render } from 'svelte/server'
import { describe, expect, it } from 'vitest'
import type { MessageContent } from '$lib/types/message'
import ToolCallsGroup from './tool-calls-group.svelte'

const content: MessageContent = [
    { id: 0, type: 'text', text: 'I checked the source.' },
    {
        id: 1,
        type: 'tool',
        status: 'completed',
        batchId: 'assistant-1',
        toolUse: {
            id: 'tool-1',
            name: 'search',
            input: { query: 'pipeline' },
        },
        toolResult: {
            toolUseId: 'tool-1',
            content: [],
        },
    },
    { id: 2, type: 'text', text: 'Here is the answer.' },
]

describe('ToolCallsGroup SSR', () => {
    it('renders completed work inside the final summary accordion', () => {
        const { body } = render(ToolCallsGroup, {
            props: {
                content,
                isStreaming: false,
                stripThinkingContent: (text: string) => text,
            },
        })

        expect(body).toContain('Worked for 0s')
        expect(body).toContain('Here is the answer.')
    })
})
