<script lang="ts">
    import type { MessageContent, TextMessageContent, ToolMessageContent } from '$lib/types/message'
    import ToolMessage from './tool-message.svelte'
    import MarkdownMessage from './markdown-message.svelte'
    import { ChevronRight } from '@lucide/svelte'
    import { fly } from 'svelte/transition'

    type Props = {
        content: MessageContent
        isStreaming: boolean
        stripThinkingContent: (text: string, tag: string) => string
    }

    const MAX_VISIBLE_TOOLS = 4

    let { content, isStreaming, stripThinkingContent }: Props = $props()
    let expanded = $state(false)

    let toolBlocks = $derived(content.filter((b): b is ToolMessageContent => b.type === 'tool'))
    let collapsibleCount = $derived(Math.max(0, toolBlocks.length - MAX_VISIBLE_TOOLS))

    // Split content into earlier (collapsible) and recent blocks
    let cutoffIndex = $derived.by(() => {
        if (collapsibleCount <= 0) return 0
        const visibleTools = new Set(toolBlocks.slice(-MAX_VISIBLE_TOOLS).map((b) => b.id))
        const idx = content.findIndex((b) => visibleTools.has(b.id))
        return idx >= 0 ? idx : 0
    })

    let earlierBlocks = $derived(collapsibleCount > 0 ? content.slice(0, cutoffIndex) : [])
    let recentBlocks = $derived(collapsibleCount > 0 ? content.slice(cutoffIndex) : content)
</script>

{#if collapsibleCount > 0}
    <button
        class="text-muted-foreground hover:text-foreground mb-3 flex cursor-pointer items-center gap-1 text-xs transition-colors"
        onclick={() => (expanded = !expanded)}>
        <ChevronRight
            class="h-3 w-3 transition-transform duration-200 {expanded ? 'rotate-90' : ''}" />
        {#if expanded}
            hide {collapsibleCount} earlier step{collapsibleCount > 1 ? 's' : ''}
        {:else}
            {collapsibleCount} earlier step{collapsibleCount > 1 ? 's' : ''}
        {/if}
    </button>

    <!-- Earlier blocks: scrollable container when expanded -->
    <div
        class="overflow-hidden transition-all duration-300 ease-in-out"
        class:max-h-0={!expanded}
        class:opacity-0={!expanded}
        class:pointer-events-none={!expanded}>
        <div class="mb-3 max-h-64 overflow-y-auto opacity-75">
            {#each earlierBlocks as block (block.id)}
                {#if block.type === 'text'}
                    <MarkdownMessage
                        content={stripThinkingContent(block.text, 'thinking')}
                        citations={block.citations} />
                {:else if block.type === 'tool'}
                    <div class="mb-1">
                        <ToolMessage message={block} />
                    </div>
                {/if}
            {/each}
        </div>
    </div>
{/if}

<!-- Recent blocks: always visible -->
{#each recentBlocks as block (block.id)}
    {#if block.type === 'text'}
        <MarkdownMessage
            content={stripThinkingContent(block.text, 'thinking')}
            citations={block.citations} />
    {:else if block.type === 'tool'}
        <div in:fly={{ y: 4, duration: 300 }} class="mb-1">
            <ToolMessage message={block} />
        </div>
    {/if}
{/each}
