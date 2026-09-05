import { describe, expect, it } from 'vitest'
import type { MessageContent, ToolMessageContent } from '$lib/types/message'
import {
    formatDuration,
    groupToolCallContent,
    isToolCallComplete,
    isToolCallFailed,
    partitionStreamingWork,
    splitToolCallContent,
    type ToolCallDisplayItem,
} from './tool-call-display'

function tool(
    id: string,
    name: string,
    batchId: string,
    input: Record<string, unknown> = {},
): ToolMessageContent {
    return {
        id: 0,
        type: 'tool',
        status: 'running',
        batchId,
        toolUse: { id, name, input },
    }
}

describe('tool call display helpers', () => {
    it('groups same-type calls emitted in one assistant iteration', () => {
        const content: MessageContent = [
            tool('search-1', 'search', 'assistant-1', { query: 'alpha' }),
            tool('search-2', 'search', 'assistant-1', { query: 'beta' }),
            tool('bash-1', 'run_bash', 'assistant-1', { command: 'pwd' }),
        ]

        const items = groupToolCallContent(content)

        expect(items).toHaveLength(2)
        expect(items[0]).toMatchObject({ type: 'tools', group: { toolName: 'search' } })
        expect(items[0].type === 'tools' && items[0].group.messages).toHaveLength(2)
        expect(items[1]).toMatchObject({ type: 'tools', group: { toolName: 'run_bash' } })
    })

    it('keeps same-type calls from later iterations separate', () => {
        const content: MessageContent = [
            tool('search-1', 'search', 'assistant-1'),
            tool('search-2', 'search', 'assistant-2'),
        ]

        const items = groupToolCallContent(content)

        expect(items).toHaveLength(2)
    })

    it('puts only text after the final tool into the final response', () => {
        const content: MessageContent = [
            { id: 0, type: 'text', text: 'I will look this up.' },
            tool('search-1', 'search', 'assistant-1'),
            { id: 2, type: 'text', text: 'Here is the answer.' },
        ]

        const parts = splitToolCallContent(content)

        expect(parts.work).toHaveLength(2)
        expect(parts.response).toEqual([{ id: 2, type: 'text', text: 'Here is the answer.' }])
    })

    it('recognizes empty tool results as complete', () => {
        const message = tool('search-1', 'search', 'assistant-1')
        expect(isToolCallComplete(message)).toBe(false)
        expect(isToolCallComplete({ ...message, status: 'completed' })).toBe(true)
        expect(isToolCallComplete({ ...message, status: 'failed' })).toBe(true)
    })

    it('uses failed as the only failure state', () => {
        const message = tool('search-1', 'search', 'assistant-1')
        expect(isToolCallFailed(message)).toBe(false)
        expect(isToolCallFailed({ ...message, status: 'failed' })).toBe(true)
    })

    it('formats elapsed work time compactly', () => {
        const start = new Date('2026-01-01T00:00:00.000Z')
        expect(formatDuration(start, new Date('2026-01-01T00:00:01.000Z'))).toBe('1s')
        expect(formatDuration(start, new Date('2026-01-01T00:01:10.000Z'))).toBe('1m 10s')
        expect(formatDuration(start, new Date('2026-01-01T01:00:00.000Z'))).toBe('1h')
    })
})

describe('partitionStreamingWork', () => {
    function groupItem(
        id: string,
        name: string,
        status: ToolMessageContent['status'] = 'completed',
    ): Extract<ToolCallDisplayItem, { type: 'tools' }> {
        return {
            type: 'tools',
            group: {
                key: `${id}:${name}`,
                toolName: name,
                messages: [{ ...tool(id, name, `assistant-${id}`, {}), status }],
            },
        }
    }

    function textItem(text: string): Extract<ToolCallDisplayItem, { type: 'text' }> {
        return { type: 'text', block: { id: 0, type: 'text', text } }
    }

    it('does not collapse while there are up to four tool groups', () => {
        const items = [
            groupItem('g1', 'search'),
            groupItem('g2', 'read_document'),
            groupItem('g3', 'search'),
            groupItem('g4', 'run_bash'),
        ]

        const partition = partitionStreamingWork(items, true)

        expect(partition.collapseActive).toBe(false)
        expect(partition.visible).toBe(items)
        expect(partition.collapsed).toHaveLength(0)
        expect(partition.previousStepsCount).toBe(0)
    })

    it('collapses everything before the three latest steps, counting completed groups', () => {
        const items = [
            textItem('First narration.'),
            groupItem('g1', 'search'),
            textItem('Between one and two.'),
            groupItem('g2', 'search'),
            groupItem('g3', 'read_document'),
            textItem('Leading into the final rounds.'),
            groupItem('g4', 'search'),
            groupItem('g5', 'run_bash'),
            groupItem('g6', 'search'),
        ]

        const partition = partitionStreamingWork(items, true)

        expect(partition.collapseActive).toBe(true)
        expect(partition.previousStepsCount).toBe(3)
        expect(partition.collapsed).toEqual([
            textItem('First narration.'),
            groupItem('g1', 'search'),
            textItem('Between one and two.'),
            groupItem('g2', 'search'),
            groupItem('g3', 'read_document'),
            textItem('Leading into the final rounds.'),
        ])
        expect(partition.visible).toEqual([
            groupItem('g4', 'search'),
            groupItem('g5', 'run_bash'),
            groupItem('g6', 'search'),
        ])
    })

    it('keeps narrations of the latest steps visible alongside the running group', () => {
        const items = [
            groupItem('g1', 'search'),
            groupItem('g2', 'search'),
            groupItem('g3', 'search'),
            groupItem('g4', 'read_document'),
            textItem('Checking the file now.'),
            groupItem('g5', 'read_file', 'running'),
        ]

        const partition = partitionStreamingWork(items, true)

        expect(partition.collapseActive).toBe(true)
        expect(partition.previousStepsCount).toBe(2)
        expect(partition.collapsed).toEqual([groupItem('g1', 'search'), groupItem('g2', 'search')])
        expect(partition.visible).toEqual([
            groupItem('g3', 'search'),
            groupItem('g4', 'read_document'),
            textItem('Checking the file now.'),
            groupItem('g5', 'read_file', 'running'),
        ])
    })

    it('never collapses output when the stream is not active', () => {
        const items = [
            groupItem('g1', 'search'),
            groupItem('g2', 'search'),
            groupItem('g3', 'search'),
            groupItem('g4', 'search'),
            groupItem('g5', 'search'),
        ]

        const partition = partitionStreamingWork(items, false)

        expect(partition.collapseActive).toBe(false)
        expect(partition.visible).toBe(items)
    })
})
