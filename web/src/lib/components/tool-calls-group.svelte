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
        isAdmin?: boolean
        onOAuthComplete?: () => void
    }

    const MAX_VISIBLE_TOOLS = 4

    let {
        content,
        isStreaming,
        stripThinkingContent,
        isAdmin = false,
        onOAuthComplete = () => {},
    }: Props = $props()
    let expanded = $state(false)

    // When the model fans out parallel tool calls against the same source and
    // they all surface oauth_required, we only want one Connect card per source.
    // Hide the duplicates here; on resume the AI service replaces every
    // placeholder so the hidden blocks become real tool results naturally.
    let skippedOAuthBlockIds = $derived.by(() => {
        const seen = new Set<string>()
        const skip = new Set<number>()
        for (const block of content) {
            if (block.type === 'tool' && block.oauthRequired) {
                const key = block.oauthRequired.sourceId
                if (seen.has(key)) skip.add(block.id)
                else seen.add(key)
            }
        }
        return skip
    })

    let visibleBlocks = $derived(
        content.filter((b) => !(b.type === 'tool' && skippedOAuthBlockIds.has(b.id))),
    )
    let toolBlocks = $derived(
        visibleBlocks.filter((b): b is ToolMessageContent => b.type === 'tool'),
    )
    let collapsibleCount = $derived(Math.max(0, toolBlocks.length - MAX_VISIBLE_TOOLS))

    // Split content into earlier (collapsible) and recent blocks
    let cutoffIndex = $derived.by(() => {
        if (collapsibleCount <= 0) return 0
        const visibleTools = new Set(toolBlocks.slice(-MAX_VISIBLE_TOOLS).map((b) => b.id))
        const idx = visibleBlocks.findIndex((b) => visibleTools.has(b.id))
        return idx >= 0 ? idx : 0
    })

    let earlierBlocks = $derived(collapsibleCount > 0 ? visibleBlocks.slice(0, cutoffIndex) : [])
    let recentBlocks = $derived(
        collapsibleCount > 0 ? visibleBlocks.slice(cutoffIndex) : visibleBlocks,
    )

    function blockRenderKey(block: MessageContent[number]): string {
        // Streamed text blocks keep the same numeric id while their markdown grows.
        // Remount just that markdown subtree so a partial parsed render cannot stay stale.
        if (block.type === 'text') {
            return `text:${block.id}:${block.text.length}:${block.citations?.length ?? 0}`
        }

        return `${block.type}:${block.id}`
    }
</script>

{#if collapsibleCount > 0}
    <button
        class="text-muted-foreground hover:text-foreground hover:bg-muted/60 mb-3 flex cursor-pointer items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors"
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
        <div class="mb-3 max-h-64 overflow-y-auto pr-1 opacity-80">
            {#each earlierBlocks as block (blockRenderKey(block))}
                {#if block.type === 'text'}
                    <MarkdownMessage
                        content={stripThinkingContent(block.text, 'thinking')}
                        citations={block.citations} />
                {:else if block.type === 'tool'}
                    <div class="mb-1">
                        <ToolMessage message={block} {isAdmin} {onOAuthComplete} />
                    </div>
                {/if}
            {/each}
        </div>
    </div>
{/if}

<!-- Recent blocks: always visible -->
{#each recentBlocks as block (blockRenderKey(block))}
    {#if block.type === 'text'}
        <MarkdownMessage
            content={stripThinkingContent(block.text, 'thinking')}
            citations={block.citations} />
    {:else if block.type === 'tool'}
        <div in:fly={{ y: 4, duration: 300 }} class="mb-1">
            <ToolMessage message={block} {isAdmin} {onOAuthComplete} />
        </div>
    {/if}
{/each}
