<script lang="ts">
    import type { PageProps } from './$types'
    import { Search } from '@lucide/svelte'
    import { goto } from '$app/navigation'
    import omniLogoLight from '$lib/images/icons/omni-logo-256.png'
    import omniLogoDark from '$lib/images/icons/omni-logo-dark-256.png'
    import UserInput, { type InputMode } from '$lib/components/user-input.svelte'
    import { userPreferences } from '$lib/preferences'

    let { data }: PageProps = $props()

    let searchQuery = $state('')
    let popoverOpen = $state(false)
    let isSearching = $state(false)
    let inputMode = $state<InputMode>(userPreferences.get('inputMode'))

    type PendingUpload = { id: string; filename: string; sizeBytes: number; uploading: boolean }
    type UploadResponse = {
        id: string
        filename: string
        content_type: string
        size_bytes: number
        created_at: string
    }
    let pendingUploads = $state<PendingUpload[]>([])
    let uploadInputEl: HTMLInputElement | undefined = $state()

    async function handleFilesSelected(files: FileList | null) {
        if (!files) return
        for (const file of Array.from(files)) {
            const placeholder: PendingUpload = {
                id: crypto.randomUUID(),
                filename: file.name,
                sizeBytes: file.size,
                uploading: true,
            }
            pendingUploads.push(placeholder)
            try {
                const fd = new FormData()
                fd.append('file', file)
                const resp = await fetch('/api/uploads', { method: 'POST', body: fd })
                if (!resp.ok) throw new Error(`upload failed: ${resp.status}`)
                const data = (await resp.json()) as UploadResponse
                const idx = pendingUploads.findIndex((u) => u.id === placeholder.id)
                if (idx >= 0) {
                    pendingUploads[idx] = {
                        id: data.id,
                        filename: data.filename,
                        sizeBytes: data.size_bytes,
                        uploading: false,
                    }
                }
            } catch (err) {
                console.error(err)
                pendingUploads = pendingUploads.filter((u) => u.id !== placeholder.id)
            }
        }
        if (uploadInputEl) uploadInputEl.value = ''
    }

    function removePendingUpload(id: string) {
        pendingUploads = pendingUploads.filter((u) => u.id !== id)
    }

    const models = $derived(data.models)

    const savedModelId = userPreferences.get('preferredModelId')
    const initialModelId = $derived.by(() => {
        if (savedModelId && models.find((m) => m.id === savedModelId)) {
            return savedModelId
        }
        const defaultModel = models.find((m) => m.isDefault)
        return defaultModel?.id ?? models[0]?.id ?? null
    })
    let selectedModelId = $state<string | null>(null)
    $effect(() => {
        selectedModelId = initialModelId
    })

    $effect(() => {
        userPreferences.set('inputMode', inputMode)
    })

    async function submitQuery() {
        const trimmed = searchQuery.trim()
        const readyAttachments = pendingUploads.filter((u) => !u.uploading)
        if (pendingUploads.some((u) => u.uploading)) return
        if (!trimmed && readyAttachments.length === 0) return

        if (isSearching) {
            return
        }

        isSearching = true

        if (inputMode === 'search') {
            goto(`/search?q=${encodeURIComponent(trimmed)}`)
            return
        }

        const response = await fetch(`/api/chat`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({ modelId: selectedModelId }),
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
                content: trimmed,
                role: 'user',
                attachmentIds: readyAttachments.map((u) => u.id),
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

    function selectSuggestion(query: string) {
        searchQuery = query
        popoverOpen = false
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
        {#if pendingUploads.length > 0 && inputMode === 'chat'}
            <div class="mb-2 flex w-full max-w-2xl flex-wrap gap-2">
                {#each pendingUploads as up (up.id)}
                    <div
                        class="bg-card flex items-center gap-2 rounded-lg border px-3 py-1.5 text-sm">
                        <span class="max-w-[200px] truncate">{up.filename}</span>
                        {#if up.uploading}
                            <span class="text-muted-foreground text-xs">uploading…</span>
                        {/if}
                        <button
                            type="button"
                            class="text-muted-foreground hover:text-foreground cursor-pointer"
                            onclick={() => removePendingUpload(up.id)}>×</button>
                    </div>
                {/each}
            </div>
        {/if}
        <div class="flex w-full max-w-2xl items-end gap-2">
            <input
                bind:this={uploadInputEl}
                type="file"
                multiple
                class="hidden"
                onchange={(e) => handleFilesSelected((e.target as HTMLInputElement).files)} />
            <div class="flex-1">
                <UserInput
                    bind:value={searchQuery}
                    bind:inputMode
                    onSubmit={submitQuery}
                    onInput={(v) => (searchQuery = v)}
                    onAttachClick={() => uploadInputEl?.click()}
                    onFilesDropped={(files) => handleFilesSelected(files)}
                    modeSelectorEnabled={true}
                    placeholders={{
                        search: 'Search for anything...',
                        chat: 'Ask anything...',
                    }}
                    isLoading={isSearching}
                    {popoverItems}
                    showPopover={popoverOpen}
                    onPopoverChange={(open) => (popoverOpen = open)}
                    maxWidth="max-w-2xl"
                    {models}
                    {selectedModelId}
                    onModelChange={(id) => {
                        selectedModelId = id
                        userPreferences.set('preferredModelId', id)
                    }} />
            </div>
        </div>

        <!-- Suggested Questions -->
        {#if data.suggestedQuestions && data.suggestedQuestions.length > 0}
            <div class="mt-8 w-full max-w-2xl">
                <div class="flex flex-col items-start gap-2">
                    <p class="text-muted-foreground text-xs font-medium uppercase">Try asking</p>
                    {#each data.suggestedQuestions as suggestion}
                        <button
                            class="hover:border-primary/20 hover:bg-muted max-w-screen-md cursor-pointer truncate rounded-full border border-gray-300 bg-white px-4 py-2 text-xs transition-colors"
                            onclick={() => selectSuggestion(suggestion.question)}>
                            {suggestion.question}
                        </button>
                    {/each}
                </div>
            </div>
        {/if}
    </div>
</div>
