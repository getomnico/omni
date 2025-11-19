<script lang="ts">
    import type { PageProps } from './$types'
    import { Search } from '@lucide/svelte'
    import { goto } from '$app/navigation'
    import omniLogoLight from '$lib/images/icons/omni-logo-256.png'
    import omniLogoDark from '$lib/images/icons/omni-logo-dark-256.png'
    import UserInput, { type InputMode } from '$lib/components/user-input.svelte'

    let { data }: PageProps = $props()

    let searchQuery = $state('')
    let popoverOpen = $state(false)
    let isSearching = $state(false)
    let inputMode = $state<InputMode>('chat')

    $inspect(inputMode)

    async function submitQuery() {
        if (searchQuery.trim() && !isSearching) {
            if (inputMode === 'search') {
                goto(`/search?q=${encodeURIComponent(searchQuery.trim())}`)
                return
            }

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

            goto(`/chat/${chatId}`, {
                invalidateAll: true,
                state: {
                    stream: true,
                },
            })
        }
    }

    function selectSuggestion(query: string) {
        searchQuery = query
        popoverOpen = false
        submitQuery()
    }

    // Map recent searches to popover items format
    const popoverItems = $derived(
        data.recentSearches?.map((query) => ({
            label: query,
            icon: Search,
            onClick: () => selectSuggestion(query),
        })) || [],
    )
</script>

<svelte:head>
    <title>Omni - Enterprise Search</title>
</svelte:head>

<div class="container mx-auto px-4">
    <!-- Centered Search Section -->
    <div class="flex min-h-[60vh] flex-col items-center justify-center">
        <div class="mb-6 flex items-center gap-2 text-center">
            <img src={omniLogoLight} alt="Omni logo" class="h-8 w-8 rounded-lg dark:hidden" />
            <img src={omniLogoDark} alt="Omni logo" class="hidden h-8 w-8 rounded-lg dark:block" />
            <h1 class="text-foreground text-3xl font-bold">omni</h1>
        </div>

        <!-- Search Box -->
        <UserInput
            bind:value={searchQuery}
            bind:inputMode
            onSubmit={submitQuery}
            onInput={(v) => (searchQuery = v)}
            modeSelectorEnabled={true}
            placeholders={{
                search: 'Search for anything...',
                chat: 'Ask anything...',
            }}
            isLoading={isSearching}
            {popoverItems}
            showPopover={popoverOpen}
            onPopoverChange={(open) => (popoverOpen = open)}
            maxWidth="max-w-2xl" />

        <!-- Suggested Questions -->
        {#if data.suggestedQuestions && data.suggestedQuestions.length > 0}
            <div class="mt-8 w-full max-w-2xl">
                <div class="flex flex-col items-center gap-2">
                    {#each data.suggestedQuestions as suggestion}
                        <button
                            class="max-w-screen-md cursor-pointer truncate rounded-full border border-gray-300 bg-white px-4 py-2 text-sm transition-colors hover:border-blue-400 hover:bg-blue-50"
                            onclick={() => selectSuggestion(suggestion.question)}>
                            {suggestion.question}
                        </button>
                    {/each}
                </div>
            </div>
        {/if}
    </div>
</div>
