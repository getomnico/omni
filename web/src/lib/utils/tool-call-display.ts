import type { MessageContent, TextMessageContent, ToolMessageContent } from '$lib/types/message'

export type ToolCallDisplayGroup = {
    key: string
    toolName: string
    messages: ToolMessageContent[]
}

export type ToolCallDisplayItem =
    | { type: 'text'; block: TextMessageContent }
    | { type: 'tools'; group: ToolCallDisplayGroup }

export type ToolCallContentParts = {
    work: MessageContent
    response: TextMessageContent[]
}

/**
 * The final assistant text is everything after the last tool call. Text before
 * a tool call is model narration and belongs to the expandable work history.
 */
export function splitToolCallContent(content: MessageContent): ToolCallContentParts {
    const lastToolIndex = content.findLastIndex((block) => block.type === 'tool')

    if (lastToolIndex === -1) {
        return {
            work: [],
            response: content.filter((block): block is TextMessageContent => block.type === 'text'),
        }
    }

    return {
        work: content.slice(0, lastToolIndex + 1),
        response: content
            .slice(lastToolIndex + 1)
            .filter((block): block is TextMessageContent => block.type === 'text'),
    }
}

/**
 * Groups tool calls emitted by the same assistant message and having the same
 * tool name. The first occurrence determines the group's position in the
 * timeline, while later result updates append to the existing group.
 */
export function groupToolCallContent(content: MessageContent): ToolCallDisplayItem[] {
    const items: ToolCallDisplayItem[] = []
    const groups = new Map<string, ToolCallDisplayGroup>()

    for (const block of content) {
        if (block.type === 'text') {
            items.push({ type: 'text', block })
            continue
        }
        if (block.type !== 'tool') continue

        const batchId = block.batchId ?? `tool:${block.toolUse.id}`
        const key = `${batchId}:${block.toolUse.name}`
        const existing = groups.get(key)
        if (existing) {
            existing.messages.push(block)
            continue
        }

        const group: ToolCallDisplayGroup = {
            key,
            toolName: block.toolUse.name,
            messages: [block],
        }
        groups.set(key, group)
        items.push({ type: 'tools', group })
    }

    return items
}

export function isToolCallComplete(message: ToolMessageContent): boolean {
    return message.status !== 'running'
}

export type StreamingWorkPartition = {
    collapseActive: boolean
    collapsed: ToolCallDisplayItem[]
    visible: ToolCallDisplayItem[]
    previousStepsCount: number
}

function isGroupComplete(group: ToolCallDisplayGroup): boolean {
    return group.messages.every((message) => isToolCallComplete(message))
}

/**
 * While a multi-round agent run is live (streaming, or paused on an
 * approval/OAuth card that keeps the run open), keep only the newest steps
 * expanded and fold older completed work into a "N previous steps" section.
 * Once the run finishes the caller switches to the single "Worked for X
 * seconds" collapse instead.
 */
export function partitionStreamingWork(
    items: ToolCallDisplayItem[],
    isLive: boolean,
): StreamingWorkPartition {
    const noCollapse: StreamingWorkPartition = {
        collapseActive: false,
        collapsed: [],
        visible: items,
        previousStepsCount: 0,
    }
    if (!isLive) return noCollapse

    const toolGroupIndexes = items
        .map((item, index) => (item.type === 'tools' ? index : -1))
        .filter((index) => index !== -1)
    // Keep up to four completed steps visible before folding older work, and
    // leave the three latest steps expanded once folding starts.
    if (toolGroupIndexes.length <= 4) return noCollapse

    const visibleStart = toolGroupIndexes[toolGroupIndexes.length - 3]
    const collapsed = items.slice(0, visibleStart)
    const visible = items.slice(visibleStart)
    const previousStepsCount = collapsed.filter(
        (item): item is Extract<ToolCallDisplayItem, { type: 'tools' }> =>
            item.type === 'tools' && isGroupComplete(item.group),
    ).length
    return { collapseActive: true, collapsed, visible, previousStepsCount }
}

export function isToolCallFailed(message: ToolMessageContent): boolean {
    return message.status === 'failed'
}

export function formatDuration(start: Date | undefined, end: Date | undefined): string {
    if (!start || !end) return '0s'

    const seconds = Math.max(0, Math.round((end.getTime() - start.getTime()) / 1000))
    if (seconds < 60) return `${seconds}s`

    const minutes = Math.floor(seconds / 60)
    const remainingSeconds = seconds % 60
    if (minutes < 60) {
        return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`
    }

    const hours = Math.floor(minutes / 60)
    const remainingMinutes = minutes % 60
    return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`
}
