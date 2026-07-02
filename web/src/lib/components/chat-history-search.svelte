<script lang="ts">
    import { goto } from '$app/navigation'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import * as Popover from '$lib/components/ui/popover/index.js'
    import type { SerializedChat, SerializedChatSearchResult } from '$lib/types/chat'
    import { cn } from '$lib/utils'
    import { formatChatTimestamp } from '$lib/utils/datetime'
    import { Bot, Search, Star } from '@lucide/svelte'

    type SearchResponse = SerializedChatSearchResult[]

    interface Props {
        currentChatId?: string
        timeZone?: string | null
    }

    let { currentChatId, timeZone }: Props = $props()

    let open = $state(false)
    let query = $state('')
    let inputRef: HTMLInputElement | null = $state(null)
    let anchorRef: HTMLDivElement | null = $state(null)

    let searchResults = $state<SerializedChatSearchResult[]>([])
    let searchLoading = $state(false)
    let searchError = $state('')
    let searchRequestId = 0
    let selectedIndex = $state(0)
    let lastTrimmedQuery = ''

    let trimmedQuery = $derived(query.trim())
    let activeItems = $derived<SerializedChat[]>(searchResults)

    async function navigateToChat(chatId: string) {
        open = false
        query = ''
        await goto(`/chat/${chatId}`)
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

    $effect(() => {
        if (!open) return

        query = ''
        searchResults = []
        searchError = ''
        selectedIndex = 0
        requestAnimationFrame(() => inputRef?.focus())
    })

    $effect(() => {
        const current = trimmedQuery
        if (current !== lastTrimmedQuery) {
            lastTrimmedQuery = current
            selectedIndex = 0
        }
    })

    $effect(() => {
        const length = activeItems.length
        if (selectedIndex >= length) {
            selectedIndex = Math.max(0, length - 1)
        }
    })

    $effect(() => {
        const current = trimmedQuery
        const requestId = ++searchRequestId

        if (!open) return

        if (!current) {
            searchResults = []
            searchError = ''
            searchLoading = false
            return
        }

        searchLoading = true
        searchError = ''

        const timeout = setTimeout(async () => {
            try {
                const response = await fetch(`/api/chat/search?q=${encodeURIComponent(current)}`)
                if (!response.ok) throw new Error('Search failed')

                const data = (await response.json()) as SearchResponse
                if (requestId === searchRequestId) {
                    searchResults = data
                }
            } catch (error) {
                if (requestId === searchRequestId) {
                    searchError = error instanceof Error ? error.message : 'Search failed'
                    searchResults = []
                }
            } finally {
                if (requestId === searchRequestId) {
                    searchLoading = false
                }
            }
        }, 250)

        return () => clearTimeout(timeout)
    })
</script>

<Popover.Root bind:open>
    <div
        bind:this={anchorRef}
        class="pointer-events-none fixed top-[18vh] left-1/2 h-0 w-0"
        aria-hidden="true">
    </div>
    <Popover.Trigger>
        {#snippet child({ props })}
            <Button
                {...props}
                variant="ghost"
                class="my-1 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                aria-label="Search chats">
                <Search />
                <span>Search Chats</span>
            </Button>
        {/snippet}
    </Popover.Trigger>
    <Popover.Content
        class="z-50 w-[min(44rem,calc(100vw-2rem))] rounded-2xl p-0 shadow-2xl"
        strategy="fixed"
        customAnchor={anchorRef}
        side="bottom"
        sideOffset={0}
        align="center"
        onOpenAutoFocus={(event) => {
            event.preventDefault()
            requestAnimationFrame(() => inputRef?.focus())
        }}>
        <div class="bg-background overflow-hidden rounded-2xl border">
            <div class="border-b p-3">
                <div class="relative">
                    <Search
                        class="text-muted-foreground pointer-events-none absolute top-1/2 left-3 h-5 w-5 -translate-y-1/2" />
                    <Input
                        bind:ref={inputRef}
                        bind:value={query}
                        type="text"
                        placeholder="Search chats..."
                        class="h-12 border-0 pr-3 pl-10 text-base shadow-none focus-visible:ring-0"
                        onkeydown={handleKeydown} />
                </div>
            </div>

            <div class="max-h-[60vh] overflow-y-auto p-2">
                {#if !trimmedQuery}
                    <div class="text-muted-foreground px-4 py-8 text-center text-sm">
                        Start typing to search chats.
                    </div>
                {:else if searchLoading}
                    <div class="text-muted-foreground px-4 py-8 text-center text-sm">
                        Searching…
                    </div>
                {:else if searchError}
                    <div class="text-destructive px-4 py-8 text-center text-sm">
                        {searchError}
                    </div>
                {:else if searchResults.length === 0}
                    <div class="text-muted-foreground px-4 py-8 text-center text-sm">
                        No chats found for “{trimmedQuery}”.
                    </div>
                {:else}
                    <div class="space-y-1" role="listbox" aria-label="Chat search results">
                        {#each searchResults as chat, index (chat.id)}
                            <button
                                type="button"
                                role="option"
                                aria-selected={index === selectedIndex}
                                class={cn(
                                    'hover:bg-accent hover:text-accent-foreground flex w-full cursor-pointer items-start gap-3 rounded-xl px-3 py-2 text-left',
                                    index === selectedIndex && 'bg-accent text-accent-foreground',
                                    currentChatId === chat.id && 'ring-ring ring-1',
                                )}
                                onmouseenter={() => (selectedIndex = index)}
                                onclick={() => navigateToChat(chat.id)}>
                                <div
                                    class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center">
                                    {#if chat.agentId}
                                        <Bot class="text-muted-foreground h-4 w-4" />
                                    {:else if chat.isStarred}
                                        <Star class="h-4 w-4 fill-current" />
                                    {:else}
                                        <Search class="text-muted-foreground h-4 w-4" />
                                    {/if}
                                </div>
                                <div class="min-w-0 flex-1">
                                    <div class="truncate text-sm font-medium">
                                        {chat.title || 'Untitled'}
                                    </div>
                                    {#if chat.snippet?.parts?.length}
                                        <div
                                            class="text-muted-foreground mt-0.5 line-clamp-2 text-xs leading-relaxed">
                                            {#each chat.snippet.parts as part, partIndex (`${chat.id}-${partIndex}`)}
                                                {#if part.match}
                                                    <strong class="text-foreground font-semibold"
                                                        >{part.text}</strong>
                                                {:else}
                                                    {part.text}
                                                {/if}
                                            {/each}
                                        </div>
                                    {/if}
                                </div>
                                <div class="text-muted-foreground shrink-0 pt-0.5 text-xs">
                                    {formatChatTimestamp(chat.updatedAt, timeZone)}
                                </div>
                            </button>
                        {/each}
                    </div>
                {/if}
            </div>
        </div>
    </Popover.Content>
</Popover.Root>
