<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import { Search, FileText } from '@lucide/svelte'
    import type { ToolMessageContent, ToolName } from '$lib/types/message'
    import { cn } from '$lib/utils'
    import { getIconFromSearchResult } from '$lib/utils/icons'

    type Props = {
        message: ToolMessageContent
    }

    const ToolIndicators = {
        search_documents: {
            loading: 'searching',
            loaded: 'searched',
        },
        read_document: {
            loading: 'reading',
            loaded: 'read',
        },
    }

    const ToolInputKey = {
        search_documents: 'query',
        read_document: 'name',
    }

    let { message }: Props = $props()
    const toolName = message.toolUse.name as ToolName
    const statusIndicator = message.toolResult
        ? ToolIndicators[toolName]?.loaded
        : ToolIndicators[toolName]?.loading || 'using'
    const toolInputKey = ToolInputKey[toolName] || 'query'

    let selectedItem = $state<string>()
</script>

<Accordion.Root
    type="single"
    bind:value={selectedItem}
    disabled={!message.toolResult || message.toolResult.content.length === 0}>
    <Accordion.Item value={message.toolUse.id}>
        <Accordion.Trigger
            class={cn(
                'flex cursor-pointer items-center justify-between border border-gray-200 px-3 py-3 text-sm hover:no-underline',
                selectedItem === message.toolUse.id && 'bg-card rounded-b-none border-b-0',
            )}>
            <div class="flex w-full items-center justify-between">
                <div class="flex items-center gap-2">
                    <Search class="h-4 w-4" />
                    <div class="max-w-screen-md truncate text-sm font-normal">
                        {statusIndicator}: {message.toolUse.input[toolInputKey]}
                    </div>
                </div>
                {#if toolName === 'search_documents' && message.toolResult}
                    <div class="text-muted-foreground text-xs">
                        {message.toolResult.content.length} results
                    </div>
                {/if}
            </div>
        </Accordion.Trigger>
        {#if toolName === 'search_documents' && message.toolResult && message.toolResult.content.length > 0}
            <Accordion.Content
                class="bg-card max-h-48 overflow-y-auto rounded-b-md border border-t-0 border-gray-200">
                <div class="px-4 py-2">
                    <div class="flex flex-col gap-2">
                        {#each message.toolResult.content as result}
                            <div class="flex items-center gap-2">
                                {#if getIconFromSearchResult(result.source)}
                                    <img
                                        src={getIconFromSearchResult(result.source)}
                                        alt=""
                                        class="!m-0 h-4 w-4 flex-shrink-0" />
                                {:else}
                                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {/if}
                                <a
                                    href={result.source.split('#')[0]}
                                    target="_blank"
                                    class="block max-w-screen-sm overflow-hidden font-normal text-ellipsis whitespace-nowrap no-underline hover:underline">
                                    {result.title}
                                </a>
                            </div>
                        {/each}
                    </div>
                </div>
            </Accordion.Content>
        {/if}
    </Accordion.Item>
</Accordion.Root>
