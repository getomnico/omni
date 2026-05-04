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
        Image,
        Download,
        Users,
        BookOpen,
    } from '@lucide/svelte'
    import type { ToolMessageContent, ToolName } from '$lib/types/message'
    import { ToolApprovalStatus } from '$lib/types/message'
    import OAuthRequiredCard from '$lib/components/oauth-required-card.svelte'
    import { cn } from '$lib/utils'
    import {
        getIconFromSearchResult,
        getSourceIconPath,
        getSourceDisplayName,
    } from '$lib/utils/icons'
    import { SourceType } from '$lib/types'

    type Props = {
        message: ToolMessageContent
        isAdmin?: boolean
        onOAuthComplete?: () => void
    }

    const ToolIndicators: Record<string, { loading: string; loaded: string }> = {
        search_documents: {
            loading: 'searching',
            loaded: 'searched',
        },
        read_document: {
            loading: 'fetching',
            loaded: 'fetched',
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
        present_artifact: {
            loading: 'presenting',
            loaded: 'presented',
        },
        search_people: {
            loading: 'searching people',
            loaded: 'searched people',
        },
        load_skill: {
            loading: 'loading skill',
            loaded: 'loaded skill',
        },
    }

    const ToolInputKey: Record<string, string> = {
        search_documents: 'query',
        read_document: 'name',
        write_file: 'path',
        read_file: 'path',
        run_bash: 'command',
        run_python: 'code',
        present_artifact: 'title',
        search_people: 'query',
        load_skill: 'skill',
    }

    const ToolApprovalColors: Record<
        ToolApprovalStatus,
        { borderColor: string; bgColor: string; color: string }
    > = {
        [ToolApprovalStatus.Pending]: {
            borderColor: 'border-warning',
            bgColor: 'bg-warning',
            color: 'text-warning-foreground',
        },
        [ToolApprovalStatus.Approved]: {
            borderColor: 'border-success',
            bgColor: 'bg-success',
            color: 'text-success-foreground',
        },
        [ToolApprovalStatus.Denied]: {
            borderColor: 'border-destructive/30',
            bgColor: 'bg-destructive/10',
            color: 'text-destructive',
        },
    }

    let { message, isAdmin = false, onOAuthComplete = () => {} }: Props = $props()
    const toolName = message.toolUse.name as ToolName

    // Determine if this is a connector action (contains __)
    const isConnectorAction = toolName.includes('__')
    const connectorSourceType = isConnectorAction ? toolName.split('__')[0] : null
    const connectorIconPath = connectorSourceType ? getSourceIconPath(connectorSourceType) : null
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
    const isSandboxTool = [
        'write_file',
        'read_file',
        'run_bash',
        'run_python',
        'present_artifact',
    ].includes(toolName)

    // Parse artifact data for present_artifact tool
    let artifactData = $derived.by(() => {
        if (toolName !== 'present_artifact' || !message.actionResult?.text) return null
        try {
            return JSON.parse(message.actionResult.text) as {
                url: string
                title: string
                content_type: string
                size_bytes: number
            }
        } catch {
            return null
        }
    })
</script>

{#if toolName === 'search_people'}
    <div
        class={cn(
            'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-2">
                <Users class="h-5 w-5 text-blue-600" />
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}: {message.toolUse.input[toolInputKey]}
                </div>
            </div>
        </div>
    </div>
{:else if toolName === 'read_document'}
    <div
        class={cn(
            'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
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
    {#if toolName === 'present_artifact' && artifactData}
        <div class="mt-2">
            {#if artifactData.content_type.startsWith('image/')}
                <figure class="border-border rounded-lg border p-2">
                    <img
                        src={artifactData.url}
                        alt={artifactData.title}
                        class="!m-0 max-w-full rounded" />
                    <figcaption class="text-muted-foreground mt-1 text-center text-xs">
                        {artifactData.title}
                    </figcaption>
                </figure>
            {:else}
                <a
                    href={artifactData.url}
                    download
                    class="border-border hover:bg-muted text-foreground inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-sm no-underline">
                    <Download class="h-4 w-4" />
                    <span>{artifactData.title}</span>
                    <span class="text-muted-foreground text-xs">
                        ({Math.round(artifactData.size_bytes / 1024)} KB)
                    </span>
                </a>
            {/if}
        </div>
    {:else}
        <div
            class={cn(
                'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
            )}>
            <div class="flex w-full items-center justify-between">
                <div class="flex items-center gap-2">
                    {#if toolName === 'present_artifact'}
                        <Image class="h-5 w-5 text-violet-600" />
                    {:else if toolName === 'run_python'}
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
    {/if}
{:else if toolName === 'load_skill'}
    <div
        class={cn(
            'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-2">
                <BookOpen class="h-5 w-5 text-indigo-600" />
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}: {message.toolUse.input[toolInputKey]}
                </div>
            </div>
        </div>
    </div>
{:else if isConnectorAction}
    {#if message.oauthRequired}
        <OAuthRequiredCard
            oauthRequired={message.oauthRequired}
            {toolName}
            {isAdmin}
            onComplete={onOAuthComplete} />
    {:else}
        <div
            class={cn(
                'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
                message.approval && ToolApprovalColors[message.approval.status]?.borderColor,
                message.approval && ToolApprovalColors[message.approval.status]?.bgColor,
            )}>
            <div class="flex w-full items-center justify-between">
                <div class="flex items-center gap-2">
                    {#if connectorIconPath}
                        <img
                            src={connectorIconPath}
                            alt={connectorSourceType}
                            class="!m-0 h-5 w-5" />
                    {:else}
                        <Play class="h-5 w-5 text-purple-600" />
                    {/if}
                    <div class="max-w-screen-md truncate text-sm font-normal">
                        {statusIndicator}: {connectorDisplayName}
                        {#if inputSummary()}
                            <span class="text-muted-foreground"> ({inputSummary()})</span>
                        {/if}
                    </div>
                </div>
                {#if message.approval}
                    <div
                        class={cn(
                            'text-xs font-medium',
                            ToolApprovalColors[message.approval.status]?.color,
                        )}>
                        {message.approval.status}
                    </div>
                {/if}
            </div>
        </div>
    {/if}
{:else}
    <Accordion.Root type="single" bind:value={selectedItem}>
        <Accordion.Item value={message.toolUse.id}>
            <Accordion.Trigger
                class={cn(
                    'border-border flex cursor-pointer items-center justify-between border px-3 py-3 text-sm hover:no-underline',
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
                    class="bg-card border-border max-h-48 overflow-y-auto rounded-b-md border border-t-0">
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
            {:else}
                <Accordion.Content class="bg-card border-border rounded-b-md border border-t-0">
                    <div class="px-4 py-2 text-center text-sm">No results found</div>
                </Accordion.Content>
            {/if}
        </Accordion.Item>
    </Accordion.Root>
{/if}
