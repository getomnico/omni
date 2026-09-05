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
        PackageSearch,
        ToolCase,
        Globe,
    } from '@lucide/svelte'
    import type { ToolMessageContent, ToolName } from '$lib/types/message'
    import { ToolApprovalStatus } from '$lib/types/message'
    import OAuthRequiredCard from '$lib/components/oauth-integrations/oauth-required-card.svelte'
    import { cn } from '$lib/utils'
    import { isToolCallComplete, isToolCallFailed } from '$lib/utils/tool-call-display'
    import {
        getIconFromSearchResult,
        getSourceIconPath,
        getSourceDisplayName,
    } from '$lib/utils/icons'
    import { SourceType } from '$lib/types'
    import { themeStore } from '$lib/themes/store.svelte'

    type Props = {
        messages: ToolMessageContent[]
        groupKey?: string
        isStreaming?: boolean
        isAdmin?: boolean
        onOAuthComplete?: () => void
        showOAuthCard?: boolean
    }

    type SearchResult = NonNullable<ToolMessageContent['toolResult']>['content'][number]

    type ArtifactData = {
        key: string
        url: string
        title: string
        content_type: string
        size_bytes: number
    }

    const ToolIndicators: Record<string, { loading: string; loaded: string }> = {
        search: { loading: 'Searching', loaded: 'Searched' },
        search_documents: { loading: 'Searching', loaded: 'Searched' },
        web_search: { loading: 'Searching web', loaded: 'Searched web' },
        fetch_web_page: { loading: 'Fetching web page', loaded: 'Fetched web page' },
        read_document: { loading: 'Reading', loaded: 'Read' },
        write_file: { loading: 'Writing', loaded: 'Wrote' },
        read_file: { loading: 'Reading', loaded: 'Read' },
        run_bash: { loading: 'Running', loaded: 'Ran' },
        run_python: { loading: 'Running', loaded: 'Ran' },
        present_artifact: { loading: 'Presenting', loaded: 'Presented' },
        search_people: { loading: 'Searching people', loaded: 'Searched people' },
        search_chats: { loading: 'Searching chats', loaded: 'Searched chats' },
        read_chat: { loading: 'Reading chat', loaded: 'Read chat' },
        tool_search: { loading: 'Searching tools', loaded: 'Searched tools' },
        load_tool: { loading: 'Loading tool', loaded: 'Loaded tool' },
        load_tool_set: { loading: 'Loading tool set', loaded: 'Loaded tool set' },
        skill_search: { loading: 'Searching skills', loaded: 'Searched skills' },
        load_skill: { loading: 'Loading skill', loaded: 'Loaded skill' },
        send_email: { loading: 'Sending email', loaded: 'Sent email' },
    }

    const ToolInputKey: Record<string, string> = {
        search: 'query',
        search_documents: 'query',
        web_search: 'query',
        fetch_web_page: 'url',
        read_document: 'name',
        write_file: 'path',
        read_file: 'path',
        run_bash: 'command',
        run_python: 'code',
        present_artifact: 'title',
        search_people: 'query',
        search_chats: 'query',
        read_chat: 'chat_id',
        tool_search: 'query',
        load_tool: 'tool_name',
        skill_search: 'query',
        load_skill: 'skill',
        send_email: 'subject',
    }

    const ToolApprovalColors: Record<ToolApprovalStatus, string> = {
        [ToolApprovalStatus.Pending]: 'text-warning-foreground',
        [ToolApprovalStatus.Approved]: 'text-success-foreground',
        [ToolApprovalStatus.Denied]: 'text-destructive',
    }

    let {
        messages,
        groupKey,
        isStreaming = false,
        isAdmin = false,
        onOAuthComplete = () => {},
        showOAuthCard = true,
    }: Props = $props()

    let primaryMessage = $derived(messages[0])
    let toolName = $derived(primaryMessage.toolUse.name as ToolName)
    let accordionKey = $derived(groupKey ?? `tool:${primaryMessage.toolUse.id}`)
    let selectedItem = $state<string>()
    let isComplete = $derived(messages.every(isToolCallComplete))
    let hasError = $derived(messages.some(isToolCallFailed))
    let needsAuth = $derived(messages.some((message) => Boolean(message.oauthRequired)))
    let isRunning = $derived(isStreaming && !needsAuth && !hasError && !isComplete)
    let isSearch = $derived(
        toolName === 'search' || toolName === 'search_documents' || toolName === 'web_search',
    )
    let isPeopleSearch = $derived(toolName === 'search_people')
    let isScript = $derived(toolName === 'run_python' || toolName === 'run_bash')
    let isConnectorAction = $derived(toolName.includes('__'))
    let isMetaTool = $derived(['tool_search', 'load_tool', 'load_tool_set'].includes(toolName))
    let isSkillTool = $derived(['skill_search', 'load_skill'].includes(toolName))
    let connectorSourceType = $derived(isConnectorAction ? toolName.split('__')[0] : null)
    let connectorIconPath = $derived(
        connectorSourceType ? getSourceIconPath(connectorSourceType) : null,
    )
    let connectorDisplayName = $derived(
        isConnectorAction ? toolName.replace('__', ' > ') : toolName,
    )
    let toolInputKey = $derived(ToolInputKey[toolName] || (isConnectorAction ? null : 'query'))

    function inputRecord(message: ToolMessageContent): Record<string, unknown> {
        const input: unknown = message.toolUse.input
        if (typeof input !== 'object' || input === null || Array.isArray(input)) return {}
        return input as Record<string, unknown>
    }

    function inputValue(message: ToolMessageContent, key: string | null): unknown {
        if (!key) return null
        return inputRecord(message)[key]
    }

    function stringInput(message: ToolMessageContent, key: string | null): string | null {
        const value = inputValue(message, key)
        return typeof value === 'string' ? value : null
    }

    function summarizeValue(value: unknown, maxLength = 80): string | null {
        if (value === null || value === undefined) return null
        const encoded = typeof value === 'string' ? value : JSON.stringify(value)
        const text = encoded ?? String(value)
        return text.length > maxLength ? `${text.substring(0, maxLength)}...` : text
    }

    function inputSummary(message: ToolMessageContent): string | null {
        if (toolName === 'load_tool_set') {
            return summarizeValue(inputValue(message, 'source_type'))
        }
        const directValue = summarizeValue(inputValue(message, toolInputKey))
        if (directValue) return directValue

        const params = Object.entries(inputRecord(message))
        if (params.length === 0) return null
        return params
            .slice(0, 2)
            .map(([key, value]) => `${key}: ${summarizeValue(value, 40) ?? 'missing'}`)
            .join(', ')
    }

    const MAX_VISIBLE_RESULT_SOURCES = 3
    let resultSources = $derived(
        Array.from(
            new Set(
                messages.flatMap((message) =>
                    (message.toolResult?.content ?? [])
                        .map((result) => result.source_type)
                        .filter((source): source is string => Boolean(source)),
                ),
            ),
        ).slice(0, MAX_VISIBLE_RESULT_SOURCES),
    )
    let resultCount = $derived(
        messages.reduce((count, message) => count + (message.toolResult?.content.length ?? 0), 0),
    )
    const MAX_VISIBLE_SEARCH_RESULTS = 5
    let statusIndicator = $derived(
        needsAuth
            ? 'Needs auth'
            : hasError
              ? 'Failed'
              : isComplete
                ? ToolIndicators[toolName]?.loaded || 'Completed'
                : ToolIndicators[toolName]?.loading || 'Running',
    )

    let summary = $derived.by(() => {
        if (toolName === 'run_python') {
            if (hasError) return 'Failed to run script'
            return isComplete ? 'Wrote script' : 'Writing script'
        }
        if (toolName === 'run_bash') {
            if (hasError) return 'Failed to run command'
            return isComplete ? 'Ran command' : 'Running command'
        }

        if (isSearch) {
            const verb = hasError
                ? 'Failed'
                : isComplete
                  ? toolName === 'web_search'
                      ? 'Searched web'
                      : 'Searched'
                  : toolName === 'web_search'
                    ? 'Searching web'
                    : 'Searching'
            if (messages.length > 1) {
                if (hasError) return `Failed after ${messages.length} searches`
                if (isComplete && resultCount > 0) return `Ran ${messages.length} searches ·`
                if (isComplete) return `Ran ${messages.length} searches`
                return `Running ${messages.length} searches`
            }
            return `${verb}: ${stringInput(primaryMessage, 'query') ?? 'missing query'}`
        }

        const detail = isConnectorAction ? connectorDisplayName : inputSummary(primaryMessage)
        if (!detail) return statusIndicator
        if (isConnectorAction) return `${statusIndicator}: ${detail}`
        if (toolName === 'read_document') return `${statusIndicator}: ${detail}`
        return selectedItem === accordionKey ? `${statusIndicator} (${detail})` : statusIndicator
    })

    function scriptText(message: ToolMessageContent): string | null {
        return stringInput(message, toolName === 'run_python' ? 'code' : 'command')
    }

    function getSearchResultIconPath(result: SearchResult): string | null {
        if (result.source_type) {
            return getSourceIconPath(result.source_type) ?? getIconFromSearchResult(result.source)
        }
        return getIconFromSearchResult(result.source)
    }

    function searchResultLabel(result: SearchResult): string {
        if (toolName === 'web_search') {
            try {
                return new URL(result.source).hostname
            } catch {
                return result.title
            }
        }
        return result.title
    }

    function parseArtifact(message: ToolMessageContent): Omit<ArtifactData, 'key'> | null {
        if (toolName !== 'present_artifact' || !message.actionResult?.text) return null
        try {
            const parsed: unknown = JSON.parse(message.actionResult.text)
            if (typeof parsed !== 'object' || parsed === null) return null
            const candidate = parsed as Record<string, unknown>
            if (
                typeof candidate.url !== 'string' ||
                typeof candidate.title !== 'string' ||
                typeof candidate.content_type !== 'string' ||
                typeof candidate.size_bytes !== 'number'
            ) {
                return null
            }
            return {
                url: candidate.url,
                title: candidate.title,
                content_type: candidate.content_type,
                size_bytes: candidate.size_bytes,
            }
        } catch {
            return null
        }
    }

    let artifactData = $derived(
        messages
            .map((message, index) => {
                const artifact = parseArtifact(message)
                return artifact ? { ...artifact, key: `${message.toolUse.id}:${index}` } : null
            })
            .filter((artifact): artifact is ArtifactData => artifact !== null),
    )

    let isArtifact = $derived(toolName === 'present_artifact' && artifactData.length > 0)
</script>

{#if isArtifact}
    {#each artifactData as artifact (artifact.key)}
        <div class="mt-2">
            {#if artifact.content_type.startsWith('image/')}
                <figure class="border-border rounded-lg border p-2">
                    <img src={artifact.url} alt={artifact.title} class="!m-0 max-w-full rounded" />
                    <figcaption class="text-muted-foreground mt-1 text-center text-xs">
                        {artifact.title}
                    </figcaption>
                </figure>
            {:else}
                <a
                    href={artifact.url}
                    download
                    rel="external"
                    class="border-border hover:bg-muted text-foreground inline-flex items-center gap-2 rounded-lg border px-3 py-2 text-sm no-underline">
                    <Download class="h-4 w-4" />
                    <span>{artifact.title}</span>
                    <span class="text-muted-foreground text-xs">
                        ({Math.round(artifact.size_bytes / 1024)} KB)
                    </span>
                </a>
            {/if}
        </div>
    {/each}
{:else}
    <Accordion.Root type="single" bind:value={selectedItem} class="tool-message-accordion">
        <Accordion.Item value={accordionKey} class="border-0 [&>h3]:m-0">
            <Accordion.Trigger
                class="text-muted-foreground hover:text-foreground inline-flex !h-8 !w-fit max-w-full flex-none cursor-pointer !items-center justify-start !gap-1.5 border-0 px-0 !py-1.5 text-sm font-normal whitespace-nowrap hover:no-underline [&:hover>svg]:opacity-100 [&>svg]:opacity-0{isRunning
                    ? ' tool-row-shimmer'
                    : ''}">
                <div class="flex min-w-0 flex-1 items-center gap-2">
                    {#if isSearch && toolName === 'web_search'}
                        <Globe class="h-4 w-4 shrink-0" />
                    {:else if isSearch && resultSources.length > 0}
                        <div class="flex shrink-0 items-center gap-1">
                            {#each resultSources as source (source)}
                                {@const sourceIcon = getSourceIconPath(source)}
                                {#if sourceIcon}
                                    <img
                                        src={sourceIcon}
                                        alt={getSourceDisplayName(source as SourceType) || source}
                                        title={getSourceDisplayName(source as SourceType) || source}
                                        class="!m-0 h-4 w-4 shrink-0" />
                                {/if}
                            {/each}
                        </div>
                    {:else if isSearch && (toolName === 'search' || toolName === 'search_documents')}
                        <img
                            src={themeStore.current.omniLogoLight}
                            alt="Omni"
                            class="omni-logo-light !m-0 h-4 w-4 shrink-0 rounded-sm" />
                        <img
                            src={themeStore.current.omniLogoDark}
                            alt="Omni"
                            class="omni-logo-dark !m-0 h-4 w-4 shrink-0 rounded-sm" />
                    {:else if isSearch}
                        <Search class="h-4 w-4 shrink-0" />
                    {:else if toolName === 'search_chats'}
                        <Search class="h-4 w-4 shrink-0" />
                    {:else if toolName === 'run_python'}
                        <FileCode class="h-4 w-4 shrink-0 text-blue-600" />
                    {:else if toolName === 'run_bash'}
                        <Terminal class="h-4 w-4 shrink-0 text-green-600" />
                    {:else if toolName === 'read_document'}
                        <TextSearch class="h-4 w-4 shrink-0" />
                    {:else if toolName === 'read_chat'}
                        <BookOpen class="h-4 w-4 shrink-0" />
                    {:else if toolName === 'search_people'}
                        <Users class="h-4 w-4 shrink-0 text-blue-600" />
                    {:else if toolName === 'write_file'}
                        <Pencil class="h-4 w-4 shrink-0 text-amber-600" />
                    {:else if toolName === 'present_artifact'}
                        <Image class="h-4 w-4 shrink-0 text-violet-600" />
                    {:else if isSkillTool}
                        <BookOpen class="h-4 w-4 shrink-0 text-indigo-600" />
                    {:else if toolName === 'send_email'}
                        <Mail class="h-4 w-4 shrink-0 text-rose-600" />
                    {:else if toolName === 'tool_search'}
                        <PackageSearch class="h-4 w-4 shrink-0 text-purple-600" />
                    {:else if isMetaTool}
                        <ToolCase class="h-4 w-4 shrink-0 text-purple-600" />
                    {:else if isConnectorAction}
                        {#if connectorIconPath}
                            <img
                                src={connectorIconPath}
                                alt={connectorSourceType}
                                class="!m-0 h-4 w-4 shrink-0" />
                        {:else}
                            <Play class="h-4 w-4 shrink-0 text-purple-600" />
                        {/if}
                    {:else}
                        <FileText class="h-4 w-4 shrink-0" />
                    {/if}
                    <span class="min-w-0 truncate text-left">{summary}</span>
                    {#if isSearch && resultCount > 0}
                        <span class="text-muted-foreground shrink-0 text-xs">
                            {resultCount}
                            {resultCount === 1 ? 'result' : 'results'}
                        </span>
                    {/if}
                </div>
            </Accordion.Trigger>
            <Accordion.Content class={cn('border-0 px-0', isSearch && 'pt-2')}>
                {#if isScript}
                    <div class="space-y-3 pl-6">
                        {#each messages as message (message.toolUse.id)}
                            <div class="space-y-1">
                                {#if messages.length > 1}
                                    <div class="text-muted-foreground text-xs">
                                        {toolName === 'run_python'
                                            ? 'Python script'
                                            : 'Bash command'}
                                    </div>
                                {/if}
                                <pre
                                    class="bg-muted/50 max-h-64 overflow-auto rounded-md p-3 text-xs leading-relaxed"><code
                                        >{scriptText(message) ??
                                            'Missing script input.'}</code></pre>
                                {#if message.actionResult}
                                    <pre
                                        class={cn(
                                            'text-foreground max-h-48 overflow-auto rounded-md p-3 text-xs leading-relaxed whitespace-pre-wrap',
                                            isToolCallFailed(message)
                                                ? 'bg-destructive/10 text-destructive'
                                                : 'bg-muted/50',
                                        )}>{message.actionResult.text}</pre>
                                {/if}
                            </div>
                        {/each}
                    </div>
                {:else if isSearch}
                    <div class="space-y-3 pl-6">
                        {#each messages as message (message.toolUse.id)}
                            <div class="space-y-2">
                                {#if messages.length > 1}
                                    <div class="text-muted-foreground text-xs">
                                        {stringInput(message, 'query') ?? 'Missing query.'}
                                    </div>
                                {/if}
                                {#if message.toolResult && message.toolResult.content.length > 0}
                                    {@const results = message.toolResult.content}
                                    {@const visibleResults = results.slice(
                                        0,
                                        MAX_VISIBLE_SEARCH_RESULTS,
                                    )}
                                    <div class="flex flex-wrap gap-1.5">
                                        {#each visibleResults as result, resultIndex (`${message.toolUse.id}:${resultIndex}`)}
                                            {@const resultIcon = getSearchResultIconPath(result)}
                                            {@const resultLabel = searchResultLabel(result)}
                                            {#if result.source.startsWith('http://') || result.source.startsWith('https://')}
                                                <a
                                                    href={result.source.split('#')[0]}
                                                    target="_blank"
                                                    rel="external noopener noreferrer"
                                                    title={result.title}
                                                    class="bg-muted/50 hover:bg-muted text-muted-foreground inline-flex max-w-56 items-center gap-1 rounded-full px-2 py-1 text-xs no-underline transition-colors">
                                                    {#if resultIcon}
                                                        <img
                                                            src={resultIcon}
                                                            alt=""
                                                            class="!m-0 h-3.5 w-3.5 shrink-0 rounded-full" />
                                                    {/if}
                                                    <span class="truncate">{resultLabel}</span>
                                                </a>
                                            {:else}
                                                <span
                                                    title={result.title}
                                                    class="bg-muted/50 text-muted-foreground inline-flex max-w-56 items-center gap-1 rounded-full px-2 py-1 text-xs">
                                                    {#if resultIcon}
                                                        <img
                                                            src={resultIcon}
                                                            alt=""
                                                            class="!m-0 h-3.5 w-3.5 shrink-0 rounded-full" />
                                                    {/if}
                                                    <span class="truncate">{resultLabel}</span>
                                                </span>
                                            {/if}
                                        {/each}
                                        {#if results.length > visibleResults.length}
                                            <span
                                                class="bg-muted/50 text-muted-foreground inline-flex items-center rounded-full px-2 py-1 text-xs">
                                                and {results.length - visibleResults.length} more
                                            </span>
                                        {/if}
                                    </div>
                                {:else if isToolCallFailed(message)}
                                    <div class="text-destructive text-xs">
                                        {message.actionResult?.text ?? 'Search failed.'}
                                    </div>
                                {:else if isToolCallComplete(message)}
                                    <div class="text-muted-foreground text-xs">
                                        No results found
                                    </div>
                                {:else}
                                    <div class="text-muted-foreground text-xs">
                                        Waiting for results...
                                    </div>
                                {/if}
                            </div>
                        {/each}
                    </div>
                {:else if isPeopleSearch}
                    <div class="space-y-3 pl-6">
                        {#each messages as message (message.toolUse.id)}
                            <div class="space-y-1 text-sm">
                                {#if message.actionResult}
                                    <pre
                                        class={cn(
                                            'text-foreground max-h-48 overflow-auto rounded-md p-3 text-xs leading-relaxed whitespace-pre-wrap',
                                            isToolCallFailed(message)
                                                ? 'bg-destructive/10 text-destructive'
                                                : 'bg-muted/50',
                                        )}>{message.actionResult.text}</pre>
                                {:else if isToolCallFailed(message)}
                                    <div class="text-destructive text-xs">
                                        People search failed.
                                    </div>
                                {:else if isToolCallComplete(message)}
                                    <div class="text-muted-foreground text-xs">
                                        No people found matching the query.
                                    </div>
                                {:else}
                                    <div class="text-muted-foreground text-xs">
                                        Waiting for results...
                                    </div>
                                {/if}
                            </div>
                        {/each}
                    </div>
                {:else}
                    <div class="space-y-3 pl-6">
                        {#each messages as message (message.toolUse.id)}
                            <div class="space-y-1 text-sm">
                                {#if message.actionResult}
                                    <pre
                                        class={cn(
                                            'text-foreground max-h-48 overflow-auto rounded-md p-3 text-xs leading-relaxed whitespace-pre-wrap',
                                            isToolCallFailed(message)
                                                ? 'bg-destructive/10 text-destructive'
                                                : 'bg-muted/50',
                                        )}>{message.actionResult.text}</pre>
                                {/if}
                                {#if message.approval}
                                    <div
                                        class={cn(
                                            'text-xs font-medium',
                                            ToolApprovalColors[message.approval.status],
                                        )}>
                                        {message.approval.status}
                                    </div>
                                {/if}
                            </div>
                        {/each}
                    </div>
                {/if}
            </Accordion.Content>
        </Accordion.Item>
    </Accordion.Root>
{/if}

{#if showOAuthCard}
    {#each messages as message (message.toolUse.id)}
        {#if message.oauthRequired}
            <div class="mt-2">
                <OAuthRequiredCard
                    oauthRequired={message.oauthRequired}
                    {toolName}
                    {isAdmin}
                    onComplete={onOAuthComplete} />
            </div>
        {/if}
    {/each}
{/if}

<style>
    :global(.tool-message-accordion [data-slot='accordion-content'][data-state='closed']) {
        display: none;
    }

    :global {
        /* A running tool call stands in for the standalone loading indicator,
           so it gets the same shine sweep while it is in progress. */
        .tool-row-shimmer {
            position: relative;
            overflow: hidden;
        }

        .tool-row-shimmer::after {
            content: '';
            position: absolute;
            top: 0;
            left: -100%;
            width: 50%;
            height: 100%;
            background: linear-gradient(
                120deg,
                transparent 0%,
                rgba(255, 255, 255, 0.6) 50%,
                transparent 100%
            );
            animation: tool-row-shine-sweep 2s ease-in-out infinite;
            pointer-events: none;
        }

        .dark .tool-row-shimmer::after {
            background: linear-gradient(
                120deg,
                transparent 0%,
                rgba(255, 255, 255, 0.3) 50%,
                transparent 100%
            );
        }

        @keyframes tool-row-shine-sweep {
            0% {
                left: -100%;
            }
            100% {
                left: 200%;
            }
        }
    }
</style>
