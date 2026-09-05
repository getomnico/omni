<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import type { MessageContent, OAuthRequired, ToolMessageContent } from '$lib/types/message'
    import ToolMessage from './tool-message.svelte'
    import MarkdownMessage from './markdown-message.svelte'
    import OAuthRequiredCard from '$lib/components/oauth-integrations/oauth-required-card.svelte'
    import {
        formatDuration,
        groupToolCallContent,
        isToolCallComplete,
        isToolCallFailed,
        partitionStreamingWork,
        splitToolCallContent,
        type ToolCallDisplayItem,
    } from '$lib/utils/tool-call-display'
    import { fly, slide } from 'svelte/transition'
    import ThinkingIndicator from './thinking-indicator.svelte'

    type Props = {
        content: MessageContent
        isStreaming: boolean
        stripThinkingContent: (text: string, tag: string) => string
        isAdmin?: boolean
        hasError?: boolean
        startedAt?: Date
        completedAt?: Date
        onOAuthComplete?: () => void
        // Rotating verb for the between-rounds waiting indicator, e.g. 'Thinking'.
        thinkingText?: string | null
        // The run is live but paused (approval/OAuth card) and the stream flag
        // is off; keep older steps folded instead of expanding the full list.
        isPaused?: boolean
    }

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
        hasError = false,
        startedAt,
        completedAt,
        onOAuthComplete = () => {},
        thinkingText = null,
        isPaused = false,
    }: Props = $props()

    let workExpanded = $state<string>()
    let contentParts = $derived(splitToolCallContent(content))
    let allWorkItems = $derived(groupToolCallContent(contentParts.work))
    let workItems = $derived(
        allWorkItems.filter((item) =>
            item.type === 'tools'
                ? item.group.toolName !== 'present_artifact'
                : stripThinkingContent(item.block.text, 'thinking').trim().length > 0,
        ),
    )
    let artifactItems = $derived(
        allWorkItems.filter(
            (item): item is Extract<ToolCallDisplayItem, { type: 'tools' }> =>
                item.type === 'tools' && item.group.toolName === 'present_artifact',
        ),
    )
    let toolBlocks = $derived(
        content.filter((block): block is ToolMessageContent => block.type === 'tool'),
    )
    let finalResponseBlocks = $derived(contentParts.response)
    let hasCollapsibleWork = $derived(workItems.length > 0)
    let hasFinalResponse = $derived(
        finalResponseBlocks.some(
            (block) => stripThinkingContent(block.text, 'thinking').trim().length > 0,
        ),
    )
    let allToolsComplete = $derived(
        toolBlocks.length > 0 && toolBlocks.every((block) => isToolCallComplete(block)),
    )
    // The model is working between rounds: every tool call has finished, and no
    // response text has started streaming yet. This replaces the per-row shimmer
    // while the agent decides what to do next.
    let awaitingModel = $derived(
        isStreaming &&
            allToolsComplete &&
            !finalResponseBlocks.some(
                (block) => stripThinkingContent(block.text, 'thinking').trim().length > 0,
            ),
    )
    let hasToolError = $derived(toolBlocks.some(isToolCallFailed))
    let canCollapseWork = $derived(
        !isStreaming &&
            !hasError &&
            !hasToolError &&
            hasFinalResponse &&
            allToolsComplete &&
            hasCollapsibleWork,
    )
    let streamingPartition = $derived(partitionStreamingWork(workItems, isStreaming || isPaused))
    let duration = $derived(formatDuration(startedAt, completedAt))

    function blockRenderKey(block: MessageContent[number]): string {
        if (block.type === 'text') return `text:${block.id}`
        return `${block.type}:${block.id}`
    }

    function workItemKey(item: ToolCallDisplayItem): string {
        return item.type === 'text' ? `text:${item.block.id}` : item.group.key
    }

    function oauthCardEntries(blocks: ToolMessageContent[]): OAuthCardEntry[] {
        const seen: string[] = []
        const entries: OAuthCardEntry[] = []
        for (const block of blocks) {
            if (!block.oauthRequired) continue
            const key = `${block.oauthRequired.sourceId}:${block.oauthRequired.provider}`
            if (seen.includes(key)) continue
            seen.push(key)
            entries.push({
                key,
                toolName: block.toolUse.name,
                oauthRequired: block.oauthRequired,
            })
        }
        return entries
    }

    function toolBlocksOf(items: ToolCallDisplayItem[]): ToolMessageContent[] {
        return items.flatMap((item) => (item.type === 'tools' ? item.group.messages : []))
    }
</script>

{#snippet workTimeline(items = workItems)}
    <div class="space-y-2">
        {#each items as item (workItemKey(item))}
            {#if item.type === 'text'}
                <div class="min-w-0 overflow-x-auto [&_ol]:my-1 [&_p]:my-1 [&_ul]:my-1">
                    <MarkdownMessage
                        content={stripThinkingContent(item.block.text, 'thinking')}
                        citations={item.block.citations}
                        {isStreaming} />
                </div>
            {:else}
                <div in:fly={{ y: 4, duration: 300 }}>
                    <ToolMessage
                        messages={item.group.messages}
                        groupKey={item.group.key}
                        {isStreaming}
                        {isAdmin}
                        {onOAuthComplete}
                        showOAuthCard={false} />
                </div>
            {/if}
        {/each}
        {#each oauthCardEntries(toolBlocksOf(items)) as entry (`oauth:${entry.key}`)}
            <div>
                <OAuthRequiredCard
                    oauthRequired={entry.oauthRequired}
                    toolName={entry.toolName}
                    {isAdmin}
                    onComplete={onOAuthComplete} />
            </div>
        {/each}
    </div>
{/snippet}

{#if canCollapseWork}
    <div transition:slide={{ duration: 200 }}>
        <Accordion.Root type="single" bind:value={workExpanded} class="tool-calls-group-accordion">
            <Accordion.Item value="work" class="border-0 [&>h3]:m-0">
                <Accordion.Trigger
                    class="text-muted-foreground hover:text-foreground inline-flex !h-8 !w-fit max-w-full flex-none cursor-pointer !items-center justify-start !gap-1.5 border-0 px-0 !py-1.5 text-sm font-normal whitespace-nowrap hover:no-underline">
                    Worked for {duration}
                </Accordion.Trigger>
                <Accordion.Content class="border-0 px-0">
                    <div>{@render workTimeline()}</div>
                </Accordion.Content>
            </Accordion.Item>
        </Accordion.Root>
    </div>
{:else}
    <div transition:slide={{ duration: 200 }}>
        <div class="space-y-2">
            {#if streamingPartition.collapseActive}
                <Accordion.Root
                    type="single"
                    bind:value={workExpanded}
                    class="tool-calls-group-accordion">
                    <Accordion.Item value="steps" class="border-0 [&>h3]:m-0">
                        <Accordion.Trigger
                            class="text-muted-foreground hover:text-foreground inline-flex !h-8 !w-fit max-w-full flex-none cursor-pointer !items-center justify-start !gap-1.5 border-0 px-0 !py-1.5 text-sm font-normal whitespace-nowrap hover:no-underline">
                            {streamingPartition.previousStepsCount} previous step{streamingPartition.previousStepsCount ===
                            1
                                ? ''
                                : 's'}
                        </Accordion.Trigger>
                        <Accordion.Content class="border-0 px-0">
                            <div>{@render workTimeline(streamingPartition.collapsed)}</div>
                        </Accordion.Content>
                    </Accordion.Item>
                </Accordion.Root>
            {/if}
            <div>{@render workTimeline(streamingPartition.visible)}</div>
            {#if awaitingModel}
                <div class="flex pt-2">
                    <ThinkingIndicator text={thinkingText ?? 'Thinking'} />
                </div>
            {/if}
        </div>
    </div>
{/if}

<div class="space-y-2">
    {#each artifactItems as item (workItemKey(item))}
        <div in:fly={{ y: 4, duration: 300 }}>
            <ToolMessage
                messages={item.group.messages}
                groupKey={item.group.key}
                {isStreaming}
                {isAdmin}
                {onOAuthComplete}
                showOAuthCard={false} />
        </div>
    {/each}
</div>

{#each finalResponseBlocks as block (blockRenderKey(block))}
    <div class="min-w-0 overflow-x-auto">
        <MarkdownMessage
            content={stripThinkingContent(block.text, 'thinking')}
            citations={block.citations}
            {isStreaming} />
    </div>
{/each}

<style>
    :global(.tool-calls-group-accordion [data-slot='accordion-content'][data-state='closed']) {
        display: none;
    }
</style>
