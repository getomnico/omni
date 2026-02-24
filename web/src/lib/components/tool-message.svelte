<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import {
        Search,
        FileText,
        TextSearch,
        ExternalLink,
        Play,
        FileCode,
        Terminal,
        Pencil,
    } from '@lucide/svelte'
    import type { ToolMessageContent, ToolName } from '$lib/types/message'
    import { cn } from '$lib/utils'
    import {
        getIconFromSearchResult,
        getSourceIconPath,
        getSourceDisplayName,
    } from '$lib/utils/icons'
    import { SourceType } from '$lib/types'

    type Props = {
        message: ToolMessageContent
    }

    const ToolIndicators: Record<string, { loading: string; loaded: string }> = {
        search_documents: {
            loading: 'searching',
            loaded: 'searched',
        },
        read_document: {
            loading: 'reading',
            loaded: 'read',
        },
        write_file: {
            loading: 'writing',
            loaded: 'wrote',
        },
        read_file: {
            loading: 'reading',
            loaded: 'read',
        },
        run_bash: {
            loading: 'running',
            loaded: 'ran',
        },
        run_python: {
            loading: 'running',
            loaded: 'ran',
        },
    }

    const ToolInputKey: Record<string, string> = {
        search_documents: 'query',
        read_document: 'name',
        write_file: 'path',
        read_file: 'path',
        run_bash: 'command',
        run_python: 'code',
    }

    let { message }: Props = $props()
    const toolName = message.toolUse.name as ToolName

    // Determine if this is a connector action (contains __)
    const isConnectorAction = toolName.includes('__')
    const connectorDisplayName = isConnectorAction ? toolName.replace('__', ' > ') : toolName

    let statusIndicator = $derived(
        message.toolResult || message.actionResult
            ? ToolIndicators[toolName]?.loaded || 'completed'
            : ToolIndicators[toolName]?.loading || 'running',
    )

    const toolInputKey = ToolInputKey[toolName] || (isConnectorAction ? '' : 'query')

    let sources = $derived<string[]>(message.toolUse.input?.sources || [])

    // Get a short summary of the tool input for display
    let inputSummary = $derived(() => {
        if (toolInputKey && message.toolUse.input?.[toolInputKey]) {
            const val = message.toolUse.input[toolInputKey]
            if (typeof val === 'string' && val.length > 80) {
                return val.substring(0, 80) + '...'
            }
            return val
        }
        // For connector actions, show a brief summary of params
        if (isConnectorAction) {
            const params = Object.entries(message.toolUse.input || {})
            if (params.length === 0) return ''
            return params
                .slice(0, 2)
                .map(([k, v]) => `${k}: ${String(v).substring(0, 40)}`)
                .join(', ')
        }
        return ''
    })

    let selectedItem = $state<string>()

    // Determine if this is a sandbox tool
    const isSandboxTool = ['write_file', 'read_file', 'run_bash', 'run_python'].includes(toolName)
</script>

{#if toolName === 'read_document'}
    <div
        class={cn(
            'flex cursor-pointer items-center justify-between rounded-md border border-gray-200 px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-2">
                <TextSearch class="h-5 w-5" />
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}: {message.toolUse.input[toolInputKey]}
                </div>
            </div>
        </div>
    </div>
{:else if isSandboxTool}
    <div
        class={cn(
            'flex cursor-pointer items-center justify-between rounded-md border border-gray-200 px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-2">
                {#if toolName === 'run_python'}
                    <FileCode class="h-5 w-5 text-blue-600" />
                {:else if toolName === 'run_bash'}
                    <Terminal class="h-5 w-5 text-green-600" />
                {:else if toolName === 'write_file'}
                    <Pencil class="h-5 w-5 text-amber-600" />
                {:else}
                    <FileText class="h-5 w-5" />
                {/if}
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}: {inputSummary()}
                </div>
            </div>
        </div>
    </div>
{:else if isConnectorAction}
    <div
        class={cn(
            'flex cursor-pointer items-center justify-between rounded-md border border-gray-200 px-3 py-3 text-sm hover:no-underline',
            message.approval?.status === 'pending' && 'border-amber-300 bg-amber-50',
            message.approval?.status === 'denied' && 'border-red-200 bg-red-50',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-2">
                <Play class="h-5 w-5 text-purple-600" />
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}: {connectorDisplayName}
                    {#if inputSummary()}
                        <span class="text-muted-foreground"> ({inputSummary()})</span>
                    {/if}
                </div>
            </div>
            {#if message.approval}
                <div
                    class="text-xs font-medium {message.approval.status === 'approved'
                        ? 'text-green-600'
                        : message.approval.status === 'denied'
                          ? 'text-red-600'
                          : 'text-amber-600'}">
                    {message.approval.status}
                </div>
            {/if}
        </div>
    </div>
{:else}
    <Accordion.Root type="single" bind:value={selectedItem}>
        <Accordion.Item value={message.toolUse.id}>
            <Accordion.Trigger
                class={cn(
                    'flex cursor-pointer items-center justify-between border border-gray-200 px-3 py-3 text-sm hover:no-underline',
                    selectedItem === message.toolUse.id && 'bg-card rounded-b-none border-b-0',
                )}>
                <div class="flex w-full items-center justify-between">
                    <div class="flex items-center gap-2">
                        {#if sources.length > 0}
                            <div class="flex items-center gap-1">
                                {#each sources as source}
                                    {#if getSourceIconPath(source)}
                                        <img
                                            src={getSourceIconPath(source)}
                                            alt={getSourceDisplayName(source as SourceType) ||
                                                source}
                                            title={getSourceDisplayName(source as SourceType) ||
                                                source}
                                            class="!m-0 h-4 w-4" />
                                    {/if}
                                {/each}
                            </div>
                        {:else}
                            <Search class="h-4 w-4" />
                        {/if}
                        <div class="max-w-screen-md truncate text-sm font-normal">
                            {#if sources.length > 0}
                                {statusIndicator}
                                {sources
                                    .map((s) => getSourceDisplayName(s as SourceType) || s)
                                    .join(', ')}: {message.toolUse.input[toolInputKey]}
                            {:else}
                                {statusIndicator}: {message.toolUse.input[toolInputKey]}
                            {/if}
                        </div>
                    </div>
                    {#if message.toolResult}
                        <div class="text-muted-foreground text-xs">
                            {message.toolResult.content.length} results
                        </div>
                    {/if}
                </div>
            </Accordion.Trigger>
            {#if message.toolResult && message.toolResult.content.length > 0}
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
                                        <FileText
                                            class="text-muted-foreground h-4 w-4 flex-shrink-0" />
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
{/if}
