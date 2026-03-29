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
    let visibleToolIds = $derived.by(() => {
        if (expanded || toolBlocks.length <= MAX_VISIBLE_TOOLS) {
            return new Set(toolBlocks.map((b) => b.id))
        }
        return new Set(toolBlocks.slice(-MAX_VISIBLE_TOOLS).map((b) => b.id))
    })
</script>

{#if collapsibleCount > 0}
    <button
        class="text-muted-foreground hover:text-foreground mb-1 flex cursor-pointer items-center gap-1 text-xs transition-colors"
        onclick={() => (expanded = !expanded)}>
        <ChevronRight
            class="h-3 w-3 transition-transform duration-200 {expanded ? 'rotate-90' : ''}" />
        {#if expanded}
            hide {collapsibleCount} tool call{collapsibleCount > 1 ? 's' : ''}
        {:else}
            {collapsibleCount} more tool call{collapsibleCount > 1 ? 's' : ''}
        {/if}
    </button>
{/if}

{#each content as block (block.id)}
    {#if block.type === 'text'}
        <MarkdownMessage
            content={stripThinkingContent(block.text, 'thinking')}
            citations={block.citations} />
    {:else if block.type === 'tool'}
        <div
            in:fly={{ y: 4, duration: 300 }}
            class="transition-all duration-300 ease-in-out"
            class:max-h-0={!visibleToolIds.has(block.id)}
            class:opacity-0={!visibleToolIds.has(block.id)}
            class:overflow-hidden={!visibleToolIds.has(block.id)}
            class:mb-0={!visibleToolIds.has(block.id)}
            class:pointer-events-none={!visibleToolIds.has(block.id)}
            class:max-h-[200px]={visibleToolIds.has(block.id)}
            class:opacity-100={visibleToolIds.has(block.id)}
            class:mb-1={visibleToolIds.has(block.id)}>
            <ToolMessage message={block} />
        </div>
    {/if}
{/each}
