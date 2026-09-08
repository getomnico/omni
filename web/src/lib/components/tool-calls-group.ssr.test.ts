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

    function artifactContent(artifactJson: string): MessageContent {
        return [
            {
                id: 0,
                type: 'tool',
                status: 'completed',
                batchId: 'assistant-1',
                toolUse: {
                    id: 'tool-artifact',
                    name: 'present_artifact',
                    input: { path: 'report.pdf', title: 'Report' },
                },
                actionResult: {
                    toolUseId: 'tool-artifact',
                    text: artifactJson,
                },
            },
            { id: 1, type: 'text', text: 'Here you go.' },
        ]
    }

    it('renders a non-image artifact as a chip with download', () => {
        const { body } = render(ToolCallsGroup, {
            props: {
                content: artifactContent(
                    JSON.stringify({
                        url: '/api/chat/chat-1/artifacts/report.pdf',
                        title: 'Quarterly Report',
                        content_type: 'application/pdf',
                        size_bytes: 2048,
                    }),
                ),
                isStreaming: false,
                stripThinkingContent: (text: string) => text,
            },
        })

        expect(body).toContain('Quarterly Report')
        expect(body).toContain('2 KB')
        expect(body).toContain('href="/api/chat/chat-1/artifacts/report.pdf"')
        expect(body).not.toContain('<img')
    })

    it('renders an image artifact inline', () => {
        const { body } = render(ToolCallsGroup, {
            props: {
                content: artifactContent(
                    JSON.stringify({
                        url: '/api/chat/chat-1/artifacts/chart.png',
                        title: 'Sales Chart',
                        content_type: 'image/png',
                        size_bytes: 4096,
                    }),
                ),
                isStreaming: false,
                stripThinkingContent: (text: string) => text,
            },
        })

        expect(body).toContain('src="/api/chat/chat-1/artifacts/chart.png"')
        expect(body).toContain('Sales Chart')
    })
})
