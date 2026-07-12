<script lang="ts">
    import type { MessageContent, OAuthRequired, ToolMessageContent } from '$lib/types/message'
    import ToolMessage from './tool-message.svelte'
    import MarkdownMessage from './markdown-message.svelte'
    import OAuthRequiredCard from '$lib/components/oauth-integrations/oauth-required-card.svelte'
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

    type OAuthCardEntry = {
        key: string
        toolName: string
        oauthRequired: OAuthRequired
    }

    let {
        content,
        isStreaming,
        stripThinkingContent,
        isAdmin = false,
        onOAuthComplete = () => {},
    }: Props = $props()
    let expanded = $state(false)

    let visibleBlocks = $derived(content)
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

    function oauthCardEntries(blocks: MessageContent): OAuthCardEntry[] {
        const seen = new Set<string>()
        const entries: OAuthCardEntry[] = []
        for (const block of blocks) {
            if (block.type !== 'tool' || !block.oauthRequired) continue
            const key = `${block.oauthRequired.sourceId}:${block.oauthRequired.provider}`
            if (seen.has(key)) continue
            seen.add(key)
            entries.push({
                key,
                toolName: block.toolUse.name,
                oauthRequired: block.oauthRequired,
            })
        }
        return entries
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
                    <div class="min-w-0 overflow-x-auto">
                        <MarkdownMessage
                            content={stripThinkingContent(block.text, 'thinking')}
                            citations={block.citations} />
                    </div>
                {:else if block.type === 'tool'}
                    <div class="mb-1">
                        <ToolMessage
                            message={block}
                            {isAdmin}
                            {onOAuthComplete}
                            showOAuthCard={false} />
                    </div>
                {/if}
            {/each}
            {#each oauthCardEntries(earlierBlocks) as entry (`oauth:${entry.key}`)}
                <div class="mt-2 mb-1">
                    <OAuthRequiredCard
                        oauthRequired={entry.oauthRequired}
                        toolName={entry.toolName}
                        {isAdmin}
                        onComplete={onOAuthComplete} />
                </div>
            {/each}
        </div>
    </div>
{/if}

<!-- Recent blocks: always visible -->
{#each recentBlocks as block (blockRenderKey(block))}
    {#if block.type === 'text'}
        <div class="min-w-0 overflow-x-auto">
            <MarkdownMessage
                content={stripThinkingContent(block.text, 'thinking')}
                citations={block.citations} />
        </div>
    {:else if block.type === 'tool'}
        <div in:fly={{ y: 4, duration: 300 }} class="mb-1">
            <ToolMessage message={block} {isAdmin} {onOAuthComplete} showOAuthCard={false} />
        </div>
    {/if}
{/each}
{#each oauthCardEntries(recentBlocks) as entry (`oauth:${entry.key}`)}
    <div in:fly={{ y: 4, duration: 300 }} class="mt-2 mb-1">
        <OAuthRequiredCard
            oauthRequired={entry.oauthRequired}
            toolName={entry.toolName}
            {isAdmin}
            onComplete={onOAuthComplete} />
    </div>
{/each}
