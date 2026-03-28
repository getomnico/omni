<script lang="ts">
    import type { MessageContent, TextMessageContent, ToolMessageContent } from '$lib/types/message'
    import ToolMessage from './tool-message.svelte'
    import MarkdownMessage from './markdown-message.svelte'
    import { ChevronRight } from '@lucide/svelte'

    type Props = {
        content: MessageContent
        isStreaming: boolean
        stripThinkingContent: (text: string, tag: string) => string
    }

    const MAX_VISIBLE_TOOLS = 4

    let { content, isStreaming, stripThinkingContent }: Props = $props()
    let expanded = $state(false)

    $effect(() => {
        if (isStreaming) expanded = false
    })

    let toolBlocks = $derived(content.filter((b): b is ToolMessageContent => b.type === 'tool'))
    let hiddenToolCount = $derived(
        expanded ? 0 : Math.max(0, toolBlocks.length - MAX_VISIBLE_TOOLS),
    )
    let visibleToolIds = $derived.by(() => {
        if (expanded || toolBlocks.length <= MAX_VISIBLE_TOOLS) {
            return new Set(toolBlocks.map((b) => b.id))
        }
        return new Set(toolBlocks.slice(-MAX_VISIBLE_TOOLS).map((b) => b.id))
    })
</script>

{#if hiddenToolCount > 0 && !isStreaming}
    <button
        class="text-muted-foreground hover:text-foreground mb-1 flex cursor-pointer items-center gap-1 text-xs transition-colors"
        onclick={() => (expanded = !expanded)}>
        <ChevronRight
            class="h-3 w-3 transition-transform duration-200 {expanded ? 'rotate-90' : ''}" />
        {hiddenToolCount} more tool call{hiddenToolCount > 1 ? 's' : ''}
    </button>
{/if}

{#each content as block (block.id)}
    {#if block.type === 'text'}
        <MarkdownMessage
            content={stripThinkingContent(block.text, 'thinking')}
            citations={block.citations} />
    {:else if block.type === 'tool'}
        <div
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
