<script lang="ts">
    import { goto } from '$app/navigation'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import * as Popover from '$lib/components/ui/popover/index.js'
    import type { Chat } from '$lib/server/db/schema'
    import { cn } from '$lib/utils'
    import { formatChatTimestamp } from '$lib/utils/datetime'
    import { Bot, Search, Star, MessageCircle } from '@lucide/svelte'
    import { onDestroy } from 'svelte'

    type ChatListItem = Omit<Chat, 'createdAt' | 'updatedAt'> & {
        createdAt: Date | string
        updatedAt: Date | string
    }

    type HighlightPart = { text: string; match: boolean }

    type ChatSearchListItem = {
        chat: ChatListItem
        titleParts: HighlightPart[]
        snippet: {
            source: 'title' | 'message'
            messageId: string | null
            parts: HighlightPart[]
        } | null
    }

    type SearchResponse = ChatSearchListItem[]

    interface Props {
        currentChatId?: string
        recentChats?: ChatListItem[]
        timeZone?: string | null
    }

    let { currentChatId, recentChats = [], timeZone }: Props = $props()

    let open = $state(false)
    let query = $state('')
    let inputRef: HTMLInputElement | null = $state(null)

    let searchResults = $state<ChatSearchListItem[]>([])
    let searchLoading = $state(false)
    let searchError = $state('')
    let selectedIndex = $state(0)
    let searchTimeout: ReturnType<typeof setTimeout> | null = null
    let searchController: AbortController | null = null

    let trimmedQuery = $derived(query.trim())
    let recentItems = $derived<ChatSearchListItem[]>(
        recentChats.map((chat) => ({
            chat,
            titleParts: [{ text: chat.title || 'Untitled', match: false }],
            snippet: null,
        })),
    )
    let visibleItems = $derived(trimmedQuery ? searchResults : recentItems)
    let activeItems = $derived<ChatListItem[]>(visibleItems.map((hit) => hit.chat))

    async function navigateToChat(chatId: string) {
        open = false
        resetSearchState()
        await goto(`/chat/${chatId}`)
    }

    function cancelSearch() {
        if (searchTimeout) {
            clearTimeout(searchTimeout)
            searchTimeout = null
        }
        searchController?.abort()
        searchController = null
    }

    function resetSearchState() {
        cancelSearch()
        query = ''
        searchResults = []
        searchError = ''
        searchLoading = false
        selectedIndex = 0
    }

    function handleOpenChange(nextOpen: boolean) {
        open = nextOpen
        resetSearchState()
    }

    function scheduleSearch(nextQuery: string) {
        cancelSearch()

        const current = nextQuery.trim()
        if (!open || !current) {
            searchResults = []
            searchError = ''
            searchLoading = false
            return
        }

        searchResults = []
        searchError = ''
        searchLoading = false

        searchTimeout = setTimeout(async () => {
            searchTimeout = null
            searchLoading = true
            const controller = new AbortController()
            searchController = controller

            try {
                const response = await fetch(`/api/chat/search?q=${encodeURIComponent(current)}`, {
                    signal: controller.signal,
                })
                if (!response.ok) throw new Error('Search failed')

                searchResults = (await response.json()) as SearchResponse
                selectedIndex = 0
            } catch (error) {
                if (!controller.signal.aborted) {
                    searchError = error instanceof Error ? error.message : 'Search failed'
                    searchResults = []
                }
            } finally {
                if (!controller.signal.aborted) {
                    searchLoading = false
                }
                if (searchController === controller) {
                    searchController = null
                }
            }
        }, 250)
    }

    function handleQueryInput(event: Event) {
        query = (event.currentTarget as HTMLInputElement).value
        selectedIndex = 0
        scheduleSearch(query)
    }

    function handleKeydown(event: KeyboardEvent) {
        if (activeItems.length === 0) return

        if (event.key === 'ArrowDown') {
            event.preventDefault()
            selectedIndex = Math.min(selectedIndex + 1, activeItems.length - 1)
        }

        if (event.key === 'ArrowUp') {
            event.preventDefault()
            selectedIndex = Math.max(selectedIndex - 1, 0)
        }

        if (event.key === 'Enter') {
            event.preventDefault()
            const selected = activeItems[selectedIndex]
            if (selected) navigateToChat(selected.id)
        }
    }

    onDestroy(cancelSearch)
</script>

<Popover.Root {open} onOpenChange={handleOpenChange}>
    <Popover.Trigger>
        {#snippet child({ props })}
            <Button
                {...props}
                variant="ghost"
                class="my-1 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                aria-label="Search chats">
                <Search />
                <span class="group-data-[collapsible=icon]:hidden">Search Chats</span>
            </Button>
        {/snippet}
    </Popover.Trigger>
    <Popover.Content
        onOpenAutoFocus={(event) => {
            event.preventDefault()
            requestAnimationFrame(() => inputRef?.focus())
        }}>
        {#snippet child({ props })}
            <div
                class="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-4">
                <div
                    {...props}
                    data-testid="chat-history-search-popover"
                    class="pointer-events-auto h-[calc(100vh/3)] w-[min(44rem,calc(100vw-2rem))] rounded-2xl p-0 shadow-2xl">
                    <div class="bg-card flex h-full flex-col overflow-hidden rounded-2xl border">
                        <div class="shrink-0 border-b p-3">
                            <div class="relative">
                                <Search
                                    class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-5 w-5 -translate-y-1/2" />
                                <Input
                                    bind:ref={inputRef}
                                    value={query}
                                    type="text"
                                    placeholder="Search chats..."
                                    class="h-12 border-0 bg-transparent pr-3 pl-10 text-base shadow-none focus-visible:ring-0"
                                    oninput={handleQueryInput}
                                    onkeydown={handleKeydown} />
                            </div>
                        </div>

                        <div class="min-h-0 flex-1 overflow-y-auto p-2">
                            {#if searchLoading}
                                <div class="text-muted-foreground px-4 py-8 text-center text-sm">
                                    Searching…
                                </div>
                            {:else if searchError}
                                <div class="text-destructive px-4 py-8 text-center text-sm">
                                    {searchError}
                                </div>
                            {:else if visibleItems.length === 0}
                                <div class="text-muted-foreground px-4 py-8 text-center text-sm">
                                    {trimmedQuery
                                        ? `No chats found for “${trimmedQuery}”.`
                                        : 'No recent chats yet.'}
                                </div>
                            {:else}
                                <div
                                    class="space-y-1"
                                    role="listbox"
                                    aria-label={trimmedQuery
                                        ? 'Chat search results'
                                        : 'Recent chats'}>
                                    {#each visibleItems as hit, index (hit.chat.id)}
                                        <button
                                            type="button"
                                            role="option"
                                            aria-selected={index === selectedIndex}
                                            class={cn(
                                                'hover:bg-accent hover:text-accent-foreground flex w-full cursor-pointer items-start gap-3 rounded-xl px-3 py-2 text-left',
                                                index === selectedIndex &&
                                                    'bg-accent text-accent-foreground',
                                                currentChatId === hit.chat.id && 'ring-ring ring-1',
                                            )}
                                            onmouseenter={() => (selectedIndex = index)}
                                            onclick={() => navigateToChat(hit.chat.id)}>
                                            <div
                                                class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center">
                                                {#if hit.chat.agentId}
                                                    <Bot class="text-muted-foreground h-4 w-4" />
                                                {:else if hit.chat.isStarred}
                                                    <Star class="text-muted-foreground h-4 w-4" />
                                                {:else}
                                                    <MessageCircle
                                                        class="text-muted-foreground h-4 w-4" />
                                                {/if}
                                            </div>
                                            <div class="min-w-0 flex-1">
                                                <div class="truncate text-sm font-medium">
                                                    {#each hit.titleParts as part, partIndex (`${hit.chat.id}-title-${partIndex}`)}
                                                        {#if part.match}
                                                            <strong class="font-semibold"
                                                                >{part.text}</strong>
                                                        {:else}
                                                            {part.text}
                                                        {/if}
                                                    {/each}
                                                </div>
                                                {#if hit.snippet?.parts?.length}
                                                    <div
                                                        class="text-muted-foreground mt-0.5 line-clamp-2 text-xs leading-relaxed">
                                                        {#each hit.snippet.parts as part, partIndex (`${hit.chat.id}-${partIndex}`)}
                                                            {#if part.match}
                                                                <strong
                                                                    class="text-foreground font-semibold"
                                                                    >{part.text}</strong>
                                                            {:else}
                                                                {part.text}
                                                            {/if}
                                                        {/each}
                                                    </div>
                                                {/if}
                                            </div>
                                            <div
                                                class="text-muted-foreground shrink-0 pt-0.5 text-xs">
                                                {formatChatTimestamp(hit.chat.updatedAt, timeZone)}
                                            </div>
                                        </button>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    </div>
                </div>
            </div>
        {/snippet}
    </Popover.Content>
</Popover.Root>
