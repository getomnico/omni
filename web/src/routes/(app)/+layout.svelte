<script lang="ts">
    import '../../app.css'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import {
        SidebarProvider,
        Sidebar,
        SidebarContent,
        SidebarHeader,
        SidebarFooter,
        SidebarGroup,
        SidebarGroupContent,
        SidebarMenu,
        SidebarMenuItem,
        SidebarMenuButton,
        SidebarMenuAction,
        SidebarTrigger,
        SidebarRail,
    } from '$lib/components/ui/sidebar/index.js'
    import {
        Tooltip,
        TooltipProvider,
        TooltipContent,
        TooltipTrigger,
    } from '$lib/components/ui/tooltip/index.js'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js'
    import * as Dialog from '$lib/components/ui/dialog/index.js'
    import type { LayoutData } from './$types.js'
    import {
        MessageCircle,
        EllipsisVertical,
        Star,
        StarOff,
        Pencil,
        Trash2,
        Bot,
    } from '@lucide/svelte'
    import { onMount, type Snippet } from 'svelte'
    import { cn } from '$lib/utils'
    import { page } from '$app/state'
    import { invalidate, invalidateAll, goto, afterNavigate } from '$app/navigation'
    import SidebarUserMenu from '$lib/components/sidebar-user-menu.svelte'
    import SidebarNavigationClose from '$lib/components/sidebar-navigation-close.svelte'
    import ChatHistorySearch from '$lib/components/chat-history-search.svelte'
    import type { Chat } from '$lib/server/db/schema'

    import { themeStore } from '$lib/themes/store.svelte'
    import { applyTheme } from '$lib/themes/engine'
    import ThemePicker from '$lib/components/theme-picker.svelte'
    import { formatDate } from '$lib/utils/datetime'

    interface Props {
        data: LayoutData
        children: Snippet
    }

    let { data, children }: Props = $props()

    type ChatDateGroup = { key: string; label: string; items: Chat[] }
    type SerializedChat = Omit<Chat, 'createdAt' | 'updatedAt'> & {
        createdAt: string
        updatedAt: string
    }
    type ChatHistoryResponse = {
        items: SerializedChat[]
        nextOffset: number | null
        hasMore: boolean
    }
    type PendingChatAction = { type: 'rename'; chat: Chat } | { type: 'delete'; chat: Chat }

    const RECENT_CHATS_PAGE_SIZE = 20

    let additionalRecentChats = $state<Chat[]>([])
    let additionalRecentHasMore = $state<boolean | null>(null)
    let recentLoadingMore = $state(false)
    let recentLoadError = $state('')
    let lastRecentChatSnapshot = ''

    let deleteTargetChat = $state<Chat | null>(null)
    let deleteTargetTitle = $state('')
    let renameTargetChat = $state<Chat | null>(null)
    let renameValue = $state('')
    let pendingChatAction = $state<PendingChatAction | null>(null)

    let isEditingHeaderTitle = $state(false)
    let headerTitleValue = $state('')
    let headerTitleInputRef: HTMLInputElement | undefined = $state()
    let optimisticTitle = $state<string | null>(null)
    let sidebarContentRef: HTMLDivElement | null = $state(null)
    let sidebarContentScrolled = $state(false)

    let currentChatTitle = $derived(
        optimisticTitle ??
            (page.url.pathname.startsWith('/chat')
                ? (page.data as { chat?: { title?: string | null } }).chat?.title
                : null),
    )
    let recentChats = $derived<Chat[]>([...data.recentChats, ...additionalRecentChats])
    let recentHasMore = $derived(additionalRecentHasMore ?? data.recentChatsHasMore)
    let recentChatGroups = $derived(groupChatsByDate(recentChats, data.user.configuration.timezone))

    function dayKey(date: Date, zone?: string | null): string {
        const parts = new Intl.DateTimeFormat('en-US', {
            year: 'numeric',
            month: '2-digit',
            day: '2-digit',
            timeZone: zone || undefined,
        }).formatToParts(date)
        const get = (type: string) => parts.find((part) => part.type === type)?.value ?? ''
        return `${get('year')}-${get('month')}-${get('day')}`
    }

    function groupLabel(date: Date, zone?: string | null): string {
        const today = new Date()
        const yesterday = new Date(today)
        yesterday.setDate(today.getDate() - 1)

        const key = dayKey(date, zone)
        if (key === dayKey(today, zone)) return 'Today'
        if (key === dayKey(yesterday, zone)) return 'Yesterday'
        return formatDate(date, zone)
    }

    function groupChatsByDate(items: Chat[], zone?: string | null): ChatDateGroup[] {
        const groups: ChatDateGroup[] = []

        for (const chat of items) {
            const date = chat.updatedAt
            const key = dayKey(date, zone)
            let group = groups[groups.length - 1]
            if (!group || group.key !== key) {
                group = { key, label: groupLabel(date, zone), items: [] }
                groups.push(group)
            }
            group.items.push(chat)
        }

        return groups
    }

    function deserializeChat(chat: SerializedChat): Chat {
        return {
            ...chat,
            createdAt: new Date(chat.createdAt),
            updatedAt: new Date(chat.updatedAt),
        }
    }

    function updateSidebarScrollState() {
        sidebarContentScrolled = (sidebarContentRef?.scrollTop ?? 0) > 0
    }

    afterNavigate(() => {
        isEditingHeaderTitle = false
        optimisticTitle = null
    })

    $effect(() => {
        // Reset appended pages when the server-provided first page changes after chat actions.
        const recentChatSnapshot = `${data.recentChats.map((chat) => chat.id).join(',')}:${data.recentChatsHasMore}`
        if (recentChatSnapshot === lastRecentChatSnapshot) return

        lastRecentChatSnapshot = recentChatSnapshot
        additionalRecentChats = []
        additionalRecentHasMore = null
        recentLoadError = ''
    })

    $effect(() => {
        const sidebarListKey = `${recentChats.length}:${data.starredChats.length}`
        requestAnimationFrame(() => {
            if (sidebarListKey) updateSidebarScrollState()
        })
    })

    async function loadMoreRecentChats() {
        if (recentLoadingMore || !recentHasMore) return

        recentLoadingMore = true
        recentLoadError = ''

        try {
            const response = await fetch(
                `/api/chat/history?limit=${RECENT_CHATS_PAGE_SIZE}&offset=${recentChats.length}&isStarred=false`,
            )
            if (!response.ok) throw new Error('Failed to load more chats')

            const data = (await response.json()) as ChatHistoryResponse
            additionalRecentChats = [...additionalRecentChats, ...data.items.map(deserializeChat)]
            additionalRecentHasMore = data.hasMore
        } catch (error) {
            recentLoadError = error instanceof Error ? error.message : 'Failed to load more chats'
        } finally {
            recentLoadingMore = false
        }
    }

    async function saveHeaderTitle() {
        const trimmed = headerTitleValue.trim()
        if (!trimmed || !page.params.chatId) {
            isEditingHeaderTitle = false
            return
        }
        optimisticTitle = trimmed
        isEditingHeaderTitle = false
        await fetch(`/api/chat/${page.params.chatId}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ title: trimmed }),
        })
        await invalidateAll()
        optimisticTitle = null
    }

    // logout is handled inside SidebarUserMenu

    async function toggleStar(chat: Chat) {
        await fetch(`/api/chat/${chat.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ isStarred: !chat.isStarred }),
        })
        invalidate('app:recent_chats')
    }

    async function confirmDelete() {
        if (!deleteTargetChat) return
        const chatId = deleteTargetChat.id
        deleteTargetChat = null

        const response = await fetch(`/api/chat/${chatId}`, { method: 'DELETE' })
        if (!response.ok) return

        if (page.params.chatId === chatId) {
            await goto('/', { invalidateAll: true })
            return
        }

        await invalidate('app:recent_chats')
    }

    async function confirmRename() {
        if (!renameTargetChat || !renameValue.trim()) return
        await fetch(`/api/chat/${renameTargetChat.id}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ title: renameValue.trim() }),
        })
        renameTargetChat = null
        renameValue = ''
        invalidate('app:recent_chats')
    }

    function openRenameDialog(chat: Chat) {
        renameTargetChat = chat
        renameValue = chat.title || ''
    }

    function runPendingChatAction() {
        if (!pendingChatAction) return

        const action = pendingChatAction
        pendingChatAction = null

        if (action.type === 'rename') {
            openRenameDialog(action.chat)
            return
        }

        deleteTargetChat = action.chat
        deleteTargetTitle = action.chat.title || 'Untitled'
    }

    async function saveDetectedTimezoneIfMissing() {
        if (data.user.configuration.timezone) return

        const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
        if (!timezone) return

        const sessionKey = `omni-timezone-detected:${data.user.id}:${timezone}`
        if (sessionStorage.getItem(sessionKey) === 'attempted') return
        sessionStorage.setItem(sessionKey, 'attempted')

        const response = await fetch('/api/user/settings', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ timezone }),
        })

        if (response.ok) {
            await invalidateAll()
        }
    }

    onMount(() => {
        updateSidebarScrollState()
        saveDetectedTimezoneIfMissing().catch(() => {
            // Timezone auto-detection is best-effort; users can still set it manually.
        })
    })

    $effect(() => {
        applyTheme(themeStore.current)
    })
</script>

<!-- Delete confirmation dialog -->
<AlertDialog.Root
    open={deleteTargetChat !== null}
    onOpenChange={(open) => {
        if (!open) deleteTargetChat = null
    }}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Delete chat</AlertDialog.Title>
            <AlertDialog.Description>
                This will permanently delete "{deleteTargetTitle}". This action cannot be undone.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDelete}>Delete</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>

<!-- Rename dialog -->
<Dialog.Root
    open={renameTargetChat !== null}
    onOpenChange={(open) => {
        if (!open) renameTargetChat = null
    }}>
    <Dialog.Content>
        <Dialog.Header>
            <Dialog.Title>Rename chat</Dialog.Title>
            <Dialog.Description>Enter a new title for this chat.</Dialog.Description>
        </Dialog.Header>
        <form
            onsubmit={(e) => {
                e.preventDefault()
                confirmRename()
            }}>
            <Input bind:value={renameValue} placeholder="Chat title" class="mb-4" />
            <Dialog.Footer>
                <Button
                    variant="outline"
                    onclick={() => {
                        renameTargetChat = null
                    }}>Cancel</Button>
                <Button type="submit">Save</Button>
            </Dialog.Footer>
        </form>
    </Dialog.Content>
</Dialog.Root>

<SidebarProvider>
    <SidebarNavigationClose />
    <!-- Chat History Sidebar -->
    <Sidebar collapsible="icon" variant="sidebar">
        <SidebarHeader class="h-16">
            <div class="flex flex-1 items-center justify-between">
                <a href="/" class="flex items-center gap-1.5 group-data-[collapsible=icon]:hidden">
                    <img
                        src={themeStore.current.omniLogoLight}
                        alt="Omni logo"
                        class="omni-logo-light ml-1 h-5 w-5 rounded-sm" />
                    <img
                        src={themeStore.current.omniLogoDark}
                        alt="Omni logo"
                        class="omni-logo-dark ml-1 h-5 w-5 rounded-sm" />
                    <span class="text-xl font-bold group-data-[collapsible=icon]:hidden"
                        >{themeStore.current.omniName}</span>
                </a>
                <TooltipProvider delayDuration={300}>
                    <Tooltip>
                        <TooltipTrigger>
                            <SidebarTrigger class="cursor-pointer" />
                        </TooltipTrigger>
                        <TooltipContent>
                            <p>Toggle sidebar</p>
                        </TooltipContent>
                    </Tooltip>
                </TooltipProvider>
            </div>
        </SidebarHeader>
        <SidebarGroup
            class={cn(
                'shrink-0 border-b border-transparent transition-[border-color,box-shadow]',
                sidebarContentScrolled && 'border-sidebar-border shadow-xs',
            )}>
            {#if data.agentsEnabled}
                <Button
                    href="/agents"
                    class="mb-2 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                    variant="ghost">
                    <Bot />
                    <span class="group-data-[collapsible=icon]:hidden">Agents</span>
                </Button>
                <hr />
            {/if}

            <Button
                href="/"
                class="mt-2 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                variant="ghost">
                <MessageCircle />
                <span class="group-data-[collapsible=icon]:hidden">New Chat</span>
            </Button>

            <!-- Chat history search -->
            <ChatHistorySearch
                currentChatId={page.params.chatId}
                recentChats={data.recentChats}
                timeZone={data.user.configuration.timezone} />
        </SidebarGroup>
        <SidebarContent bind:ref={sidebarContentRef} onscroll={updateSidebarScrollState}>
            <SidebarGroup>
                <SidebarGroupContent>
                    <!-- Starred chats -->
                    {#if data.starredChats.length > 0}
                        <p
                            class="text-muted-foreground mt-2 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
                            Starred
                        </p>
                        <SidebarMenu class="gap-1 group-data-[collapsible=icon]:hidden">
                            {#each data.starredChats as chat (chat.id)}
                                {@render chatItem(chat)}
                            {/each}
                        </SidebarMenu>
                    {/if}

                    <!-- Recent chats -->
                    <p
                        class="text-muted-foreground mt-2 p-1.5 text-xs font-semibold group-data-[collapsible=icon]:hidden">
                        Recent chats
                    </p>
                    {#if recentChats.length > 0}
                        {#each recentChatGroups as group (group.key)}
                            <p
                                class="text-muted-foreground mt-2 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
                                {group.label}
                            </p>
                            <SidebarMenu class="gap-1 group-data-[collapsible=icon]:hidden">
                                {#each group.items as chat (chat.id)}
                                    {@render chatItem(chat)}
                                {/each}
                            </SidebarMenu>
                        {/each}
                        {#if recentHasMore}
                            <Button
                                variant="ghost"
                                size="sm"
                                class="mt-2 w-full cursor-pointer text-xs group-data-[collapsible=icon]:hidden"
                                disabled={recentLoadingMore}
                                onclick={loadMoreRecentChats}>
                                {recentLoadingMore ? 'Loading...' : 'Load more'}
                            </Button>
                        {/if}
                        {#if recentLoadError}
                            <p
                                class="text-destructive px-2 py-1 text-xs group-data-[collapsible=icon]:hidden">
                                {recentLoadError}
                            </p>
                        {/if}
                    {:else if data.starredChats.length === 0}
                        <div
                            class="text-muted-foreground px-3 py-4 text-center text-sm group-data-[collapsible=icon]:hidden">
                            No chats yet
                        </div>
                    {/if}
                </SidebarGroupContent>
            </SidebarGroup>
        </SidebarContent>
        <SidebarFooter>
            <SidebarUserMenu
                email={data.user.email}
                isAdmin={data.user.role === 'admin'}
                memoryEnabled={data.memoryEnabled} />
        </SidebarFooter>
        <SidebarRail />
    </Sidebar>

    <!-- Main content area -->
    <div class="flex max-h-[100vh] w-full min-w-0 flex-1 flex-col">
        <header class={cn('bg-background sticky top-0 z-50 transition-shadow')}>
            <div class="flex h-16 w-full items-center justify-between px-3 sm:px-6">
                <div class="text-foreground flex h-16 min-w-0 flex-1 items-center">
                    <SidebarTrigger class="mr-1 size-11 shrink-0 cursor-pointer md:hidden" />
                    <div class="min-w-0 flex-1 overflow-hidden px-2 text-base font-medium sm:px-4">
                        {#if page.url.pathname === '/search'}
                            Search
                        {:else if page.url.pathname.startsWith('/chat') && currentChatTitle}
                            {#if isEditingHeaderTitle}
                                <input
                                    bind:this={headerTitleInputRef}
                                    bind:value={headerTitleValue}
                                    class="text-foreground border-border w-full border-b bg-transparent outline-none"
                                    onkeydown={(e) => {
                                        if (e.key === 'Enter') saveHeaderTitle()
                                        if (e.key === 'Escape') {
                                            isEditingHeaderTitle = false
                                        }
                                    }}
                                    onblur={() => saveHeaderTitle()} />
                            {:else}
                                <button
                                    class="text-foreground block w-full cursor-pointer truncate text-left transition-opacity hover:opacity-70"
                                    onclick={() => {
                                        isEditingHeaderTitle = true
                                        headerTitleValue = currentChatTitle || ''
                                        requestAnimationFrame(() => headerTitleInputRef?.focus())
                                    }}>
                                    {currentChatTitle}
                                </button>
                            {/if}
                        {:else if page.url.pathname.startsWith('/chat')}
                            Chat
                        {:else if page.url.pathname.startsWith('/agents')}
                            Agents
                        {:else}
                            <!-- empty -->
                        {/if}
                    </div>
                </div>
                <div class="flex items-center gap-2">
                    <ThemePicker class="h-8 w-8 lg:h-9 lg:w-9" />
                </div>
            </div>
        </header>

        <!-- Main content -->
        <main class="min-h-0 flex-1">
            {@render children()}
        </main>
    </div>
</SidebarProvider>

{#snippet chatItem(chat: Chat)}
    <SidebarMenuItem>
        <SidebarMenuButton
            class={cn(
                page.params.chatId === chat.id &&
                    'bg-sidebar-accent text-sidebar-accent-foreground',
            )}>
            {#snippet child({ props })}
                <a href="/chat/{chat.id}" {...props}>
                    <div class="flex items-center gap-1.5 truncate">
                        {#if chat.agentId}
                            <Bot class="text-muted-foreground h-3.5 w-3.5 shrink-0" />
                        {/if}
                        <span class="truncate">{chat.title || 'Untitled'}</span>
                    </div>
                </a>
            {/snippet}
        </SidebarMenuButton>
        <DropdownMenu.Root
            onOpenChangeComplete={(open) => {
                if (!open) runPendingChatAction()
            }}>
            <DropdownMenu.Trigger>
                {#snippet child({ props })}
                    <SidebarMenuAction {...props} showOnHover class="cursor-pointer">
                        <EllipsisVertical class="h-4 w-4" />
                    </SidebarMenuAction>
                {/snippet}
            </DropdownMenu.Trigger>
            <DropdownMenu.Content side="right" align="start">
                <DropdownMenu.Item onclick={() => toggleStar(chat)} class="cursor-pointer">
                    {#if chat.isStarred}
                        <StarOff class="h-4 w-4" />
                        <span>Unstar</span>
                    {:else}
                        <Star class="h-4 w-4" />
                        <span>Star</span>
                    {/if}
                </DropdownMenu.Item>
                <DropdownMenu.Item
                    onSelect={() => {
                        pendingChatAction = { type: 'rename', chat }
                    }}
                    class="cursor-pointer">
                    <Pencil class="h-4 w-4" />
                    <span>Rename</span>
                </DropdownMenu.Item>
                <DropdownMenu.Separator />
                <DropdownMenu.Item
                    class="text-destructive focus:text-destructive cursor-pointer"
                    onSelect={() => {
                        pendingChatAction = { type: 'delete', chat }
                    }}>
                    <Trash2 class="h-4 w-4" />
                    <span>Delete</span>
                </DropdownMenu.Item>
            </DropdownMenu.Content>
        </DropdownMenu.Root>
    </SidebarMenuItem>
{/snippet}
