<script lang="ts">
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card'
    import { Button } from '$lib/components/ui/button'
    import * as Popover from '$lib/components/ui/popover'
    import type { PageProps } from './$types'
    import { Input } from '$lib/components/ui/input'
    import { Search, Clock, History, Loader2, Send } from '@lucide/svelte'
    import { goto } from '$app/navigation'
    import { cn } from '$lib/utils'

    let { data }: PageProps = $props()

    let searchQuery = $state('')
    let popoverOpen = $state(false)
    let popoverContainer: HTMLDivElement | undefined = $state()
    let isSearching = $state(false)

    async function handleSearch() {
        console.log('calling handleSearch', searchQuery)

        if (searchQuery.trim() && !isSearching) {
            const response = await fetch(`/api/chat`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
            })

            if (!response.ok) {
                console.error('Failed to create chat session')
                return
            }

            const { chatId } = await response.json()
            console.log('Created chat session with ID:', chatId)

            const msgResponse = await fetch(`/api/chat/${chatId}/messages`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: searchQuery.trim(),
                    role: 'user',
                }),
            })

            if (!msgResponse.ok) {
                console.error('Failed to send message to chat session')
                return
            }

            const { messageId } = await msgResponse.json()
            console.log('Sent message with ID:', messageId)

            isSearching = true
            popoverOpen = false

            if (!data.aiFirstSearchEnabled) {
                goto(`/search?q=${encodeURIComponent(searchQuery.trim())}`)
            } else {
                goto(`/chat/${chatId}`, {
                    invalidateAll: true,
                    state: {
                        stream: true,
                    },
                })
            }
        }
    }

    function handleKeyPress(event: KeyboardEvent) {
        console.log('handle key press')
        if (event.key === 'Enter') {
            handleSearch()
        }
    }

    function selectRecentSearch(query: string) {
        console.log('select recent search')
        searchQuery = query
        popoverOpen = false
        handleSearch()
    }

    function handleFocus(e: any) {
        console.log('handle focus')
        if (data.recentSearches && data.recentSearches.length > 0) {
            popoverOpen = true
        }
    }

    function handleBlur() {
        console.log('handle blur')
        popoverOpen = false
    }
</script>

<svelte:head>
    <title>Omni - Enterprise Search</title>
</svelte:head>

<div class="container mx-auto px-4">
    <!-- Centered Search Section -->
    <div class="flex min-h-[60vh] flex-col items-center justify-center">
        <div class="mb-8 text-center">
            <h1 class="text-foreground mb-4 text-4xl font-bold">Welcome to Omni</h1>
            <p class="text-muted-foreground text-lg">
                Your AI copilot for work. Ask questions, create content, and get things done.
            </p>
        </div>

        <!-- Search Box -->
        <div class="w-full max-w-2xl" bind:this={popoverContainer}>
            <div
                class={cn(
                    'flex items-center border border-gray-200 bg-white shadow-sm',
                    popoverOpen ? 'rounded-t-xl' : 'rounded-xl',
                )}>
                <Input
                    type="text"
                    bind:value={searchQuery}
                    placeholder="Ask anything..."
                    class="text-md md:text-md flex-1 border-none bg-transparent px-4 shadow-none focus:border-none focus:ring-0 focus:outline-none focus-visible:ring-0 focus-visible:ring-offset-0"
                    onkeypress={handleKeyPress}
                    onfocus={handleFocus}
                    onblur={handleBlur} />
                <Button
                    class="m-2 cursor-pointer rounded-lg px-6 py-2"
                    onclick={handleSearch}
                    disabled={!searchQuery.trim() || isSearching}>
                    {#if isSearching}
                        <Loader2 class="h-4 w-4 animate-spin" />
                    {:else}
                        <Send class="h-4 w-4" />
                    {/if}
                </Button>
            </div>
            <div class="" bind:this={popoverContainer}></div>

            <Popover.Root open={popoverOpen}>
                {#if data.recentSearches && data.recentSearches.length > 0}
                    <Popover.Content
                        class="w-[42rem] max-w-2xl rounded-b-xl p-0"
                        align="start"
                        sideOffset={-1}
                        trapFocus={false}
                        customAnchor={popoverContainer}
                        onOpenAutoFocus={(e) => {
                            e.preventDefault()
                        }}
                        onCloseAutoFocus={(e) => {
                            e.preventDefault()
                        }}
                        onFocusOutside={(e) => e.preventDefault()}>
                        <div class="w-full border bg-white">
                            <div class="py-2">
                                {#each data.recentSearches as recentQuery}
                                    <button
                                        class="hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground w-full px-4 py-2.5 text-left text-sm transition-colors focus:outline-none"
                                        onclick={() => selectRecentSearch(recentQuery)}>
                                        <div class="flex items-center gap-3">
                                            <Clock class="text-muted-foreground h-4 w-4" />
                                            <span class="font-semibold text-violet-500"
                                                >{recentQuery}</span>
                                        </div>
                                    </button>
                                {/each}
                            </div>
                        </div>
                    </Popover.Content>
                {/if}
            </Popover.Root>
        </div>

        <!-- Suggested Questions -->
        {#if data.suggestedQuestions && data.suggestedQuestions.length > 0}
            <div class="mt-8 w-full max-w-2xl">
                <div class="flex flex-col items-center gap-2">
                    {#each data.suggestedQuestions as suggestion}
                        <button
                            class="cursor-pointer truncate rounded-full border border-gray-300 bg-white px-4 py-2 text-sm transition-colors hover:border-violet-400 hover:bg-violet-50"
                            onclick={() => selectRecentSearch(suggestion.question)}>
                            {suggestion.question}
                        </button>
                    {/each}
                </div>
            </div>
        {/if}
    </div>
</div>
