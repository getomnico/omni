<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import { ArrowLeft, Loader2, Trash2, RefreshCw, Plus } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import TelegramConnectorSetup from '$lib/components/telegram-connector-setup.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import telegramLogo from '$lib/images/icons/telegram.svg'
    import { toast } from 'svelte-sonner'

    interface ChatInfo {
        id: number
        title: string
        type: string
        username?: string
        participants_count?: number
        message_count?: number
        unread_count?: number
    }

    let { data }: PageProps = $props()

    let enabled = $state(data.source.isActive)
    const sourceConfig = data.source.config as Record<string, any> | null
    const configuredChats: string[] = (sourceConfig?.chats as string[]) ?? []
    // Map of chat name → source name that already syncs it
    const otherSyncedChats: Record<string, string> = data.otherSyncedChats ?? {}

    let isSubmitting = $state(false)
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let showRemoveDialog = $state(false)
    let showAddAccountDialog = $state(false)

    // Chat selection state
    let availableChats = $state<ChatInfo[]>([])
    let selectedChatNames = $state<Set<string>>(new Set(configuredChats))
    let isLoadingChats = $state(false)
    let chatsLoaded = $state(false)
    let chatLoadError = $state<string | null>(null)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null
    let originalEnabled = data.source.isActive

    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }

        window.addEventListener('beforeunload', beforeUnloadHandler)

        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) {
                cancel()
            }
        }
    })

    $effect(() => {
        const currentChats = JSON.stringify([...selectedChatNames].sort())
        const originalChats = JSON.stringify([...configuredChats].sort())
        hasUnsavedChanges = enabled !== originalEnabled || currentChats !== originalChats
    })

    async function loadChats() {
        isLoadingChats = true
        chatLoadError = null
        try {
            const response = await fetch(`/api/sources/${data.source.id}/action`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ action: 'list_chats', params: {} }),
            })

            if (!response.ok) {
                const err = await response.json().catch(() => null)
                throw new Error(err?.message || err?.error || 'Failed to load chats')
            }

            // Connector-manager returns: { error: null|string, result: { chats: [...] }, status: 'success'|'failure' }
            const body = await response.json()
            if (body.error) {
                throw new Error(body.error)
            }
            const chats = body.result?.chats
            if (!Array.isArray(chats)) {
                throw new Error('Unexpected response format: missing result.chats')
            }
            availableChats = chats
            chatsLoaded = true
        } catch (error: any) {
            console.error('Error loading chats:', error)
            chatLoadError = error.message || 'Failed to load chats'
            toast.error(chatLoadError)
        } finally {
            isLoadingChats = false
        }
    }

    function toggleChat(chatTitle: string) {
        const updated = new Set(selectedChatNames)
        if (updated.has(chatTitle)) {
            updated.delete(chatTitle)
        } else {
            updated.add(chatTitle)
        }
        selectedChatNames = updated
    }

    function selectAll() {
        selectedChatNames = new Set(availableChats.map((c) => c.title))
    }

    function selectNew() {
        selectedChatNames = new Set(
            availableChats.filter((c) => !otherSyncedChats[c.title]).map((c) => c.title),
        )
    }

    function selectNone() {
        selectedChatNames = new Set()
    }

    function getChatTypeLabel(type: string): string {
        const labels: Record<string, string> = {
            group: 'Group',
            supergroup: 'Supergroup',
            channel: 'Channel',
            private: 'Private',
            user: 'Private',
            bot: 'Bot',
        }
        return labels[type] ?? type
    }

    function getMemberCount(chat: ChatInfo): number | null {
        return chat.participants_count ?? chat.message_count ?? null
    }
</script>

<svelte:head>
    <title>Configure Telegram - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-4">
        <div class="flex items-center justify-between">
            <a
                href="/admin/settings/integrations"
                class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors">
                <ArrowLeft class="h-4 w-4" />
                Back to Integrations
            </a>
            <Button
                variant="outline"
                size="sm"
                class="cursor-pointer"
                onclick={() => (showAddAccountDialog = true)}>
                <Plus class="mr-1 h-4 w-4" />
                Add Another Account
            </Button>
        </div>

        <form
            method="POST"
            use:enhance={() => {
                isSubmitting = true
                return async ({ result, update }) => {
                    if (result.type === 'redirect') {
                        skipUnsavedCheck = true
                        hasUnsavedChanges = false

                        if (beforeUnloadHandler) {
                            window.removeEventListener('beforeunload', beforeUnloadHandler)
                            beforeUnloadHandler = null
                        }
                    }

                    await update()
                    isSubmitting = false
                }
            }}>
            <input
                type="hidden"
                name="selected_chats"
                value={JSON.stringify([...selectedChatNames])} />

            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={telegramLogo} alt="Telegram" class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index messages from Telegram chats, groups, and channels
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="enabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="enabled"
                                bind:checked={enabled}
                                name="enabled"
                                class="cursor-pointer" />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content class="space-y-4">
                    <!-- Chat Selection -->
                    <div class="space-y-3">
                        <div class="flex items-center justify-between">
                            <div>
                                <h3 class="text-sm font-medium">Chat Selection</h3>
                                <p class="text-muted-foreground text-xs">
                                    {#if configuredChats.length > 0}
                                        {configuredChats.length} chat{configuredChats.length === 1
                                            ? ''
                                            : 's'} configured
                                    {:else}
                                        No chats selected (will sync all chats)
                                    {/if}
                                </p>
                            </div>
                            <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                class="cursor-pointer"
                                disabled={isLoadingChats}
                                onclick={loadChats}>
                                {#if isLoadingChats}
                                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                                    Loading...
                                {:else}
                                    <RefreshCw class="mr-2 h-4 w-4" />
                                    {chatsLoaded ? 'Refresh' : 'Load Chats'}
                                {/if}
                            </Button>
                        </div>

                        {#if chatLoadError}
                            <p class="text-sm text-red-500">{chatLoadError}</p>
                        {/if}

                        {#if chatsLoaded && availableChats.length > 0}
                            {@const hasOverlap = availableChats.some(
                                (c) => otherSyncedChats[c.title],
                            )}
                            <div class="flex flex-wrap gap-2">
                                <Button
                                    type="button"
                                    variant="ghost"
                                    size="sm"
                                    class="cursor-pointer text-xs"
                                    onclick={selectAll}>
                                    Select All
                                </Button>
                                {#if hasOverlap}
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="sm"
                                        class="cursor-pointer text-xs"
                                        onclick={selectNew}>
                                        Select New Only
                                    </Button>
                                {/if}
                                <Button
                                    type="button"
                                    variant="ghost"
                                    size="sm"
                                    class="cursor-pointer text-xs"
                                    onclick={selectNone}>
                                    Select None
                                </Button>
                                <span class="text-muted-foreground self-center text-xs">
                                    {selectedChatNames.size} of {availableChats.length} selected
                                </span>
                            </div>
                            {#if hasOverlap}
                                <p class="text-xs text-amber-600 dark:text-amber-400">
                                    Some chats are already synced by another Telegram source.
                                    Selecting them here will create duplicate indexed messages.
                                </p>
                            {/if}

                            <div
                                class="max-h-80 space-y-1 overflow-y-auto rounded-md border p-2">
                                {#each availableChats as chat}
                                    {@const syncedBy = otherSyncedChats[chat.title]}
                                    <label
                                        class="hover:bg-muted flex cursor-pointer items-center gap-3 rounded-md px-2 py-1.5"
                                        class:opacity-60={syncedBy && !selectedChatNames.has(chat.title)}>
                                        <input
                                            type="checkbox"
                                            checked={selectedChatNames.has(chat.title)}
                                            onchange={() => toggleChat(chat.title)}
                                            class="accent-primary" />
                                        <div class="min-w-0 flex-1">
                                            <div class="flex items-center gap-1.5 truncate">
                                                <span class="text-sm font-medium"
                                                    >{chat.title}</span>
                                                {#if chat.username}
                                                    <span class="text-muted-foreground text-xs">
                                                        @{chat.username}</span>
                                                {/if}
                                            </div>
                                            {#if syncedBy}
                                                <span
                                                    class="text-xs text-amber-600 dark:text-amber-400">
                                                    Already synced by "{syncedBy}"
                                                </span>
                                            {/if}
                                        </div>
                                        <div class="flex shrink-0 items-center gap-2">
                                            {#if getMemberCount(chat)}
                                                <span class="text-muted-foreground text-xs">
                                                    {getMemberCount(chat)?.toLocaleString()} members
                                                </span>
                                            {/if}
                                            <span
                                                class="bg-muted text-muted-foreground rounded px-1.5 py-0.5 text-xs">
                                                {getChatTypeLabel(chat.type)}
                                            </span>
                                        </div>
                                    </label>
                                {/each}
                            </div>
                        {:else if chatsLoaded}
                            <p class="text-muted-foreground text-sm">
                                No chats found. Make sure the session is authenticated and has
                                access to chats.
                            </p>
                        {/if}
                    </div>
                </Card.Content>

                <Card.Footer class="flex justify-end">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !hasUnsavedChanges}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                        {/if}
                        Save Configuration
                    </Button>
                </Card.Footer>
            </Card.Root>
        </form>

        <Card.Root>
            <Card.Content class="flex items-center justify-between">
                <div>
                    <Card.Title>Delete Source</Card.Title>
                    <Card.Description>
                        Permanently delete this source and all its synced documents, credentials,
                        and sync history.
                    </Card.Description>
                </div>
                <Button
                    variant="destructive"
                    class="cursor-pointer"
                    onclick={() => (showRemoveDialog = true)}>
                    <Trash2 class="mr-2 h-4 w-4" />
                    Delete Permanently
                </Button>
            </Card.Content>
        </Card.Root>

        <RemoveSourceDialog
            bind:open={showRemoveDialog}
            sourceId={data.source.id}
            sourceName={data.source.name} />
    </div>
</div>

<TelegramConnectorSetup
    bind:open={showAddAccountDialog}
    onSuccess={() => {
        showAddAccountDialog = false
        window.location.href = '/admin/settings/integrations'
    }}
    onCancel={() => (showAddAccountDialog = false)} />
