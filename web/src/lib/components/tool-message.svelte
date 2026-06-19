<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import {
        Search,
        FileText,
        TextSearch,
        Play,
        FileCode,
        Terminal,
        Pencil,
        Image,
        Download,
        Users,
        BookOpen,
        Mail,
    } from '@lucide/svelte'
    import type { ToolMessageContent, ToolName } from '$lib/types/message'
    import { ToolApprovalStatus } from '$lib/types/message'
    import OAuthRequiredCard from '$lib/components/oauth-integrations/oauth-required-card.svelte'
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
        tool_search: {
            loading: 'searching tools',
            loaded: 'searched tools',
        },
        load_tool: {
            loading: 'loading tool',
            loaded: 'loaded tool',
        },
        load_tool_set: {
            loading: 'loading tool set',
            loaded: 'loaded tool set',
        },
        skill_search: {
            loading: 'searching skills',
            loaded: 'searched skills',
        },
        load_skill: {
            loading: 'loading skill',
            loaded: 'loaded skill',
        },
        send_email: {
            loading: 'sending email',
            loaded: 'sent email',
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
        tool_search: 'query',
        load_tool: 'tool_name',
        skill_search: 'query',
        load_skill: 'skill',
        send_email: 'subject',
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
    let toolName = $derived(message.toolUse.name as ToolName)

    // Determine if this is a connector action (contains __)
    let isConnectorAction = $derived(toolName.includes('__'))
    let connectorSourceType = $derived(isConnectorAction ? toolName.split('__')[0] : null)
    let connectorIconPath = $derived(
        connectorSourceType ? getSourceIconPath(connectorSourceType) : null,
    )
    let connectorDisplayName = $derived(
        isConnectorAction ? toolName.replace('__', ' > ') : toolName,
    )

    let statusIndicator = $derived(
        message.oauthRequired
            ? 'needs auth'
            : message.actionResult?.isError
              ? 'failed'
              : message.toolResult || message.actionResult
                ? ToolIndicators[toolName]?.loaded || 'completed'
                : ToolIndicators[toolName]?.loading || 'running',
    )

    let toolInputKey = $derived(ToolInputKey[toolName] || (isConnectorAction ? '' : 'query'))

    let sources = $derived<string[]>(message.toolUse.input?.sources || [])

    const summarizeValue = (value: unknown, maxLength = 80): string => {
        if (value === null || value === undefined) return ''
        const encoded = typeof value === 'string' ? value : JSON.stringify(value)
        const text = encoded ?? String(value)
        return text.length > maxLength ? text.substring(0, maxLength) + '...' : text
    }

    // Get a short summary of the tool input for display
    let inputSummary = $derived(() => {
        if (toolName === 'load_tool_set') {
            return (
                summarizeValue(message.toolUse.input?.source_type) ||
                summarizeValue(message.toolUse.input?.source_id)
            )
        }
        if (toolInputKey && message.toolUse.input?.[toolInputKey]) {
            return summarizeValue(message.toolUse.input[toolInputKey])
        }
        const params = Object.entries(message.toolUse.input || {})
        if (params.length === 0) return ''
        return params
            .slice(0, 2)
            .map(([k, v]) => `${k}: ${summarizeValue(v, 40)}`)
            .join(', ')
    })

    let selectedItem = $state<string>()

    const getSearchResultIconPath = (result: { source: string; source_type?: string | null }) => {
        if (result.source_type) {
            return getSourceIconPath(result.source_type) ?? getIconFromSearchResult(result.source)
        }
        return getIconFromSearchResult(result.source)
    }

    // Determine if this is a sandbox tool
    let isSandboxTool = $derived(
        ['write_file', 'read_file', 'run_bash', 'run_python', 'present_artifact'].includes(
            toolName,
        ),
    )

    let isMetaTool = $derived(['tool_search', 'load_tool', 'load_tool_set'].includes(toolName))
    let isSkillTool = $derived(['skill_search', 'load_skill'].includes(toolName))

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
            'border-border flex w-full min-w-0 cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 flex-1 items-center gap-2">
                <Users class="h-5 w-5 shrink-0 text-blue-600" />
                <div class="min-w-0 truncate text-sm font-normal">
                    {statusIndicator}: {inputSummary()}
                </div>
            </div>
        </div>
    </div>
{:else if toolName === 'read_document'}
    <div
        class={cn(
            'border-border flex w-full min-w-0 cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 flex-1 items-center gap-2">
                <TextSearch class="h-5 w-5 shrink-0" />
                <div class="min-w-0 truncate text-sm font-normal">
                    {statusIndicator}: {inputSummary()}
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
                'border-border flex w-full min-w-0 cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
            )}>
            <div class="flex w-full items-center justify-between">
                <div class="flex min-w-0 flex-1 items-center gap-2">
                    {#if toolName === 'present_artifact'}
                        <Image class="h-5 w-5 shrink-0 text-violet-600" />
                    {:else if toolName === 'run_python'}
                        <FileCode class="h-5 w-5 shrink-0 text-blue-600" />
                    {:else if toolName === 'run_bash'}
                        <Terminal class="h-5 w-5 shrink-0 text-green-600" />
                    {:else if toolName === 'write_file'}
                        <Pencil class="h-5 w-5 shrink-0 text-amber-600" />
                    {:else}
                        <FileText class="h-5 w-5 shrink-0" />
                    {/if}
                    <div class="min-w-0 truncate text-sm font-normal">
                        {statusIndicator}: {inputSummary()}
                    </div>
                </div>
            </div>
        </div>
    {/if}
{:else if isSkillTool}
    <div
        class={cn(
            'border-border flex w-full min-w-0 cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 flex-1 items-center gap-2">
                <BookOpen class="h-5 w-5 shrink-0 text-indigo-600" />
                <div class="min-w-0 truncate text-sm font-normal">
                    {statusIndicator}: {inputSummary()}
                </div>
            </div>
        </div>
    </div>
{:else if isConnectorAction}
    <div
        class={cn(
            'border-border flex w-full min-w-0 cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
            message.approval && ToolApprovalColors[message.approval.status]?.borderColor,
            message.approval && ToolApprovalColors[message.approval.status]?.bgColor,
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 flex-1 items-center gap-2">
                {#if connectorIconPath}
                    <img
                        src={connectorIconPath}
                        alt={connectorSourceType}
                        class="!m-0 h-5 w-5 shrink-0" />
                {:else}
                    <Play class="h-5 w-5 shrink-0 text-purple-600" />
                {/if}
                <div class="min-w-0 truncate text-sm font-normal">
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
    {#if message.oauthRequired}
        <div class="mt-2">
            <OAuthRequiredCard
                oauthRequired={message.oauthRequired}
                {toolName}
                {isAdmin}
                onComplete={onOAuthComplete} />
        </div>
    {/if}
{:else if isMetaTool || toolName === 'send_email'}
    <div
        class={cn(
            'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
            message.approval && ToolApprovalColors[message.approval.status]?.borderColor,
            message.approval && ToolApprovalColors[message.approval.status]?.bgColor,
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 items-center gap-2">
                {#if toolName === 'send_email'}
                    <Mail class="h-5 w-5 text-rose-600" />
                {:else if toolName === 'tool_search'}
                    <Search class="h-5 w-5 text-purple-600" />
                {:else}
                    <Play class="h-5 w-5 text-purple-600" />
                {/if}
                <div class="max-w-screen-md truncate text-sm font-normal">
                    {statusIndicator}{#if inputSummary()}: {inputSummary()}{/if}
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
{:else if toolName === 'search_documents'}
    <Accordion.Root type="single" bind:value={selectedItem}>
        <Accordion.Item value={message.toolUse.id}>
            <Accordion.Trigger
                class={cn(
                    'border-border flex w-full min-w-0 cursor-pointer items-center justify-between border px-3 py-3 text-sm hover:no-underline',
                    selectedItem === message.toolUse.id && 'bg-card rounded-b-none border-b-0',
                )}>
                <div class="flex w-full items-center justify-between">
                    <div class="flex min-w-0 flex-1 items-center gap-2">
                        {#if sources.length > 0}
                            <div class="flex shrink-0 items-center gap-1">
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
                            <Search class="h-4 w-4 shrink-0" />
                        {/if}
                        <div class="min-w-0 truncate text-sm font-normal">
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
                                <div class="flex min-w-0 items-center gap-2">
                                    {#if getSearchResultIconPath(result)}
                                        <img
                                            src={getSearchResultIconPath(result)}
                                            alt=""
                                            class="!m-0 h-4 w-4 flex-shrink-0" />
                                    {:else}
                                        <FileText
                                            class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                    {/if}
                                    {#if result.source?.startsWith('http://') || result.source?.startsWith('https://')}
                                        <a
                                            href={result.source.split('#')[0]}
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            class="block min-w-0 flex-1 truncate font-normal no-underline hover:underline">
                                            {result.title}
                                        </a>
                                    {:else}
                                        <span class="block min-w-0 flex-1 truncate font-normal">
                                            {result.title}
                                        </span>
                                    {/if}
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
{:else}
    <div
        class={cn(
            'border-border flex cursor-pointer items-center justify-between rounded-md border px-3 py-3 text-sm hover:no-underline',
            message.approval && ToolApprovalColors[message.approval.status]?.borderColor,
            message.approval && ToolApprovalColors[message.approval.status]?.bgColor,
        )}>
        <div class="flex w-full items-center justify-between">
            <div class="flex min-w-0 items-center gap-2">
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
