<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import { Search } from '@lucide/svelte'
    import type { ToolMessageContent } from '$lib/types/message'
    import { cn } from '$lib/utils'

    type Props = {
        message: ToolMessageContent
    }

    let { message }: Props = $props()
    let selectedItem = $state<string>()
</script>

<Accordion.Root type="single" bind:value={selectedItem}>
    <Accordion.Item value={message.toolUse.id}>
        <Accordion.Trigger
            class={cn(
                'flex cursor-pointer items-center justify-between border border-gray-200 px-2 py-2 text-sm hover:no-underline',
                selectedItem === message.toolUse.id && 'bg-card rounded-b-none border-b-0',
            )}>
            <div class="flex w-full items-center justify-between">
                <div class="flex items-center gap-2">
                    <Search class="h-4 w-4" />
                    <div class="text-sm font-normal">
                        searched: {message.toolUse.input.query}
                    </div>
                </div>
                <div class="text-muted-foreground text-xs">
                    {message.toolResult ? message.toolResult.content.length : 0} results
                </div>
            </div>
        </Accordion.Trigger>
        {#if message.toolResult && message.toolResult.content.length > 0}
            <Accordion.Content
                class="bg-card max-h-48 overflow-y-auto rounded-b-md border border-t-0 border-gray-200">
                <div class="px-4 py-2">
                    <div class="flex flex-col gap-2">
                        {#each message.toolResult.content as result}
                            <div class="">
                                <a
                                    href={result.source}
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
