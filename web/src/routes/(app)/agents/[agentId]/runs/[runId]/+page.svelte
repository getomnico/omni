<script lang="ts">
    import { Badge } from '$lib/components/ui/badge/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import { ArrowLeft } from '@lucide/svelte'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import type { ToolMessageContent } from '$lib/types/message'
    import type { PageData } from './$types.js'

    let { data }: { data: PageData } = $props()

    const TEXT_COLLAPSE_THRESHOLD = 500

    let expandedBlocks = $state(new Set<string>())

    function toggleExpand(key: string) {
        const next = new Set(expandedBlocks)
        if (next.has(key)) {
            next.delete(key)
        } else {
            next.add(key)
        }
        expandedBlocks = next
    }

    type ProcessedBlock =
        | { type: 'text'; text: string }
        | { type: 'tool'; content: ToolMessageContent }

    type ProcessedLogEntry = {
        role: string
        blocks: ProcessedBlock[]
    }

    // Map of tool_use id -> ToolMessageContent reference, for pairing with tool_result
    let toolBlocksById = new Map<string, ToolMessageContent>()

    function processExecutionLog(log: any[]): ProcessedLogEntry[] {
        const entries: ProcessedLogEntry[] = []
        toolBlocksById = new Map()

        for (const message of log) {
            const role = message.role || 'unknown'
            const content = message.content
            const blocks: ProcessedBlock[] = []

            if (typeof content === 'string') {
                blocks.push({ type: 'text', text: content })
            } else if (Array.isArray(content)) {
                for (const block of content) {
                    if (block.type === 'text') {
                        blocks.push({ type: 'text', text: block.text })
                    } else if (block.type === 'tool_use') {
                        const toolMsg: ToolMessageContent = {
                            id: 0,
                            type: 'tool',
                            toolUse: {
                                id: block.id,
                                name: block.name,
                                input: block.input,
                            },
                        }
                        toolBlocksById.set(block.id, toolMsg)
                        blocks.push({ type: 'tool', content: toolMsg })
                    } else if (block.type === 'tool_result') {
                        const toolUseId = block.tool_use_id
                        const toolMsg = toolBlocksById.get(toolUseId)
                        if (!toolMsg) continue

                        const resultContent = Array.isArray(block.content) ? block.content : []

                        // Extract search results
                        const searchResults = resultContent.filter(
                            (b: any) => b.type === 'search_result',
                        )
                        if (searchResults.length > 0) {
                            toolMsg.toolResult = {
                                toolUseId,
                                content: searchResults.map((r: any) => ({
                                    title: r.title,
                                    source: r.source,
                                })),
                            }
                        }

                        // Extract text results
                        const textBlocks = resultContent.filter((b: any) => b.type === 'text')
                        if (textBlocks.length > 0) {
                            toolMsg.actionResult = {
                                toolUseId,
                                text: textBlocks.map((b: any) => b.text).join('\n'),
                                isError: block.is_error || false,
                            }
                        }

                        // Don't add a separate block for tool_result — it's merged into the tool_use block
                    }
                }
            }

            if (blocks.length > 0) {
                entries.push({ role, blocks })
            }
        }

        return entries
    }

    let processedLog = $derived(processExecutionLog(data.run.executionLog || []))

    function formatDate(date: Date | string | null): string {
        if (!date) return '—'
        return new Date(date).toLocaleString()
    }

    function statusColor(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
        switch (status) {
            case 'completed':
                return 'default'
            case 'running':
                return 'secondary'
            case 'failed':
                return 'destructive'
            default:
                return 'outline'
        }
    }
</script>

<div class="mx-auto max-w-4xl p-6">
    <Button variant="ghost" href="/agents/{data.agent.id}" class="mb-4 cursor-pointer">
        <ArrowLeft class="mr-1 h-4 w-4" /> Back to {data.agent.name}
    </Button>

    <div class="mb-6">
        <div class="flex items-center gap-3">
            <h1 class="text-2xl font-bold">Run Details</h1>
            <Badge variant={statusColor(data.run.status)}>{data.run.status}</Badge>
        </div>
        <div class="text-muted-foreground mt-2 space-y-1 text-sm">
            <p>Started: {formatDate(data.run.startedAt)}</p>
            <p>Completed: {formatDate(data.run.completedAt)}</p>
        </div>
    </div>

    {#if data.run.summary}
        <div class="bg-muted/30 mb-6 rounded-lg border p-4">
            <h3 class="mb-2 font-medium">Summary</h3>
            <p class="text-sm">{data.run.summary}</p>
        </div>
    {/if}

    {#if data.run.errorMessage}
        <div
            class="mb-6 rounded-lg border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950">
            <h3 class="mb-2 font-medium text-red-700 dark:text-red-400">Error</h3>
            <p class="text-sm text-red-600 dark:text-red-400">{data.run.errorMessage}</p>
        </div>
    {/if}

    {#if processedLog.length > 0}
        <h2 class="mb-4 text-lg font-semibold">Execution Log</h2>
        <div class="space-y-3">
            {#each processedLog as entry, msgIdx}
                <div class="rounded-lg border p-3">
                    <div class="text-muted-foreground mb-2 text-xs font-medium uppercase">
                        {entry.role}
                    </div>
                    <div class="space-y-2">
                        {#each entry.blocks as block, blockIdx}
                            {#if block.type === 'text'}
                                {@const key = `${msgIdx}-${blockIdx}`}
                                {@const isLong = block.text.length > TEXT_COLLAPSE_THRESHOLD}
                                {@const isExpanded = expandedBlocks.has(key)}
                                <pre class="text-sm whitespace-pre-wrap">{isLong && !isExpanded
                                        ? block.text.slice(0, TEXT_COLLAPSE_THRESHOLD) + '…'
                                        : block.text}</pre>
                                {#if isLong}
                                    <button
                                        class="text-muted-foreground hover:text-foreground cursor-pointer text-xs underline"
                                        onclick={() => toggleExpand(key)}>
                                        {isExpanded ? 'Show less' : 'Show more'}
                                    </button>
                                {/if}
                            {:else if block.type === 'tool'}
                                <ToolMessage message={block.content} />
                            {/if}
                        {/each}
                    </div>
                </div>
            {/each}
        </div>
    {/if}
</div>
