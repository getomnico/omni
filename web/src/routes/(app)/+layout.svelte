<script lang="ts">
    import '../../app.css'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import {
        SidebarProvider,
        Sidebar,
        SidebarContent,
        SidebarHeader,
        SidebarGroup,
        SidebarGroupContent,
        SidebarGroupLabel,
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
        LogOut,
        MessageCirclePlus,
        Settings,
        Ellipsis,
        Star,
        StarOff,
        Pencil,
        Trash2,
        Search,
        X,
    } from '@lucide/svelte'
    import type { Snippet } from 'svelte'
    import { cn } from '$lib/utils'
    import { page } from '$app/state'
    import { invalidate, goto } from '$app/navigation'
    import * as Avatar from '$lib/components/ui/avatar'
    import type { Chat } from '$lib/server/db/schema'

    interface Props {
        data: LayoutData
        children: Snippet
    }

    let { data, children }: Props = $props()

    let searchQuery = $state('')
    let searchResults = $state<Chat[]>([])
    let isSearching = $state(false)
    let searchTimeout: ReturnType<typeof setTimeout> | undefined

    let deleteTargetChat = $state<Chat | null>(null)
    let renameTargetChat = $state<Chat | null>(null)
    let renameValue = $state('')

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }

    function handleSearchInput(value: string) {
        searchQuery = value
        clearTimeout(searchTimeout)

        if (!value.trim()) {
            searchResults = []
            isSearching = false
            return
        }

        isSearching = true
        searchTimeout = setTimeout(async () => {
            try {
                const res = await fetch(`/api/chat/search?q=${encodeURIComponent(value.trim())}`)
                if (res.ok) {
                    searchResults = await res.json()
                }
            } catch {
                // silently fail
            } finally {
                isSearching = false
            }
        }, 300)
    }

    function clearSearch() {
        searchQuery = ''
        searchResults = []
        isSearching = false
    }

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

        await fetch(`/api/chat/${chatId}`, { method: 'DELETE' })

        if (page.params.chatId === chatId) {
            goto('/')
        }
        invalidate('app:recent_chats')
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

    const displayedChats = $derived(searchQuery.trim() ? searchResults : [])
    const showSearchResults = $derived(searchQuery.trim().length > 0)
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
                This will permanently delete "{deleteTargetChat?.title || 'Untitled'}". This action
                cannot be undone.
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
    <!-- Chat History Sidebar -->
    <Sidebar collapsible="icon" variant="sidebar">
        <SidebarHeader class="h-16">
            <div class="flex flex-1 items-center justify-start gap-2">
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
                <a href="/" class="flex items-center group-data-[collapsible=icon]:hidden">
                    <span class="text-xl font-bold group-data-[collapsible=icon]:hidden">omni</span>
                </a>
            </div>
        </SidebarHeader>
        <SidebarContent class="flex flex-col">
            <SidebarGroup class="flex-1">
                <Button
                    href="/"
                    class="my-2 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                    variant="ghost">
                    <MessageCirclePlus />
                    <span class="group-data-[collapsible=icon]:hidden">New Chat</span>
                </Button>

                <!-- Search input -->
                <div class="relative my-1 group-data-[collapsible=icon]:hidden">
                    <Search
                        class="text-muted-foreground pointer-events-none absolute top-1/2 left-2 h-4 w-4 -translate-y-1/2" />
                    <Input
                        type="text"
                        placeholder="Search chats..."
                        value={searchQuery}
                        oninput={(e) => handleSearchInput(e.currentTarget.value)}
                        class="h-8 pr-8 pl-8 text-xs" />
                    {#if searchQuery}
                        <button
                            class="text-muted-foreground hover:text-foreground absolute top-1/2 right-2 -translate-y-1/2 cursor-pointer"
                            onclick={clearSearch}>
                            <X class="h-3.5 w-3.5" />
                        </button>
                    {/if}
                </div>

                <SidebarGroupContent>
                    {#if showSearchResults}
                        <!-- Search results -->
                        <p
                            class="text-muted-foreground mt-4 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
                            {isSearching
                                ? 'Searching...'
                                : `${displayedChats.length} result${displayedChats.length !== 1 ? 's' : ''}`}
                        </p>
                        <SidebarMenu class="gap-1 group-data-[collapsible=icon]:hidden">
                            {#each displayedChats as chat (chat.id)}
                                <SidebarMenuItem>
                                    <SidebarMenuButton
                                        class={cn(
                                            page.params.chatId === chat.id &&
                                                'bg-sidebar-accent text-sidebar-accent-foreground',
                                        )}>
                                        {#snippet child({ props })}
                                            <a
                                                href="/chat/{chat.id}"
                                                {...props}
                                                onclick={clearSearch}>
                                                <div class="flex items-center gap-1.5 truncate">
                                                    {#if chat.isStarred}
                                                        <Star
                                                            class="h-3 w-3 shrink-0 fill-current" />
                                                    {/if}
                                                    {chat.title || 'Untitled'}
                                                </div>
                                            </a>
                                        {/snippet}
                                    </SidebarMenuButton>
                                </SidebarMenuItem>
                            {/each}
                        </SidebarMenu>
                    {:else}
                        <!-- Starred chats -->
                        {#if data.starredChats.length > 0}
                            <p
                                class="text-muted-foreground mt-4 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
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
                            class="text-muted-foreground mt-4 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
                            Recent chats
                        </p>
                        <SidebarMenu class="gap-1 group-data-[collapsible=icon]:hidden">
                            {#if data.recentChats.length > 0}
                                {#each data.recentChats as chat (chat.id)}
                                    {@render chatItem(chat)}
                                {/each}
                            {:else if data.starredChats.length === 0}
                                <div
                                    class="text-muted-foreground px-3 py-4 text-center text-sm group-data-[collapsible=icon]:hidden">
                                    No chats yet
                                </div>
                            {/if}
                        </SidebarMenu>
                    {/if}
                </SidebarGroupContent>
            </SidebarGroup>
            <SidebarGroup>
                <div class="flex flex-col gap-1">
                    {#if data.user.role === 'admin'}
                        <div class="flex justify-start">
                            <Button
                                variant="ghost"
                                href="/admin/settings"
                                class="flex w-full justify-start has-[>svg]:px-2">
                                <Settings />
                                <span class="group-data-[collapsible=icon]:hidden">Settings</span>
                            </Button>
                        </div>
                    {/if}
                    <div class="flex justify-between py-2">
                        <div class="flex min-w-0 flex-1 items-center gap-1.5">
                            <Avatar.Root>
                                <Avatar.Fallback
                                    >{data.user.email
                                        .slice(0, 2)
                                        .toLocaleUpperCase()}</Avatar.Fallback>
                            </Avatar.Root>
                            <span
                                class="text-muted-foreground truncate overflow-hidden text-sm group-data-[collapsible=icon]:hidden">
                                {data.user.email}
                            </span>
                        </div>
                        <TooltipProvider delayDuration={300}>
                            <Tooltip>
                                <TooltipTrigger>
                                    <Button
                                        size="icon"
                                        variant="ghost"
                                        class="cursor-pointer group-data-[collapsible=icon]:hidden"
                                        onclick={logout}>
                                        <LogOut class="h-4 w-4" />
                                    </Button>
                                </TooltipTrigger>
                                <TooltipContent>
                                    <p>Logout</p>
                                </TooltipContent>
                            </Tooltip>
                        </TooltipProvider>
                    </div>
                </div>
            </SidebarGroup>
        </SidebarContent>
        <SidebarRail />
    </Sidebar>

    <!-- Main content area -->
    <div class="flex max-h-[100vh] w-full min-w-0 flex-1 flex-col">
        <header class={cn('bg-background sticky top-0 z-50 transition-shadow')}>
            <div class="prose flex h-16 items-center px-6">
                <h2 class="px-4">
                    {page.url.pathname === '/search'
                        ? 'Search'
                        : page.url.pathname.startsWith('/chat')
                          ? 'Chat'
                          : ''}
                </h2>
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
                    <div class="truncate">
                        {chat.title || 'Untitled'}
                    </div>
                </a>
            {/snippet}
        </SidebarMenuButton>
        <DropdownMenu.Root>
            <DropdownMenu.Trigger>
                {#snippet child({ props })}
                    <SidebarMenuAction {...props} showOnHover>
                        <Ellipsis class="h-4 w-4" />
                    </SidebarMenuAction>
                {/snippet}
            </DropdownMenu.Trigger>
            <DropdownMenu.Content side="right" align="start">
                <DropdownMenu.Item onclick={() => toggleStar(chat)}>
                    {#if chat.isStarred}
                        <StarOff class="h-4 w-4" />
                        <span>Unstar</span>
                    {:else}
                        <Star class="h-4 w-4" />
                        <span>Star</span>
                    {/if}
                </DropdownMenu.Item>
                <DropdownMenu.Item onclick={() => openRenameDialog(chat)}>
                    <Pencil class="h-4 w-4" />
                    <span>Rename</span>
                </DropdownMenu.Item>
                <DropdownMenu.Separator />
                <DropdownMenu.Item
                    class="text-destructive focus:text-destructive"
                    onclick={() => {
                        deleteTargetChat = chat
                    }}>
                    <Trash2 class="h-4 w-4" />
                    <span>Delete</span>
                </DropdownMenu.Item>
            </DropdownMenu.Content>
        </DropdownMenu.Root>
    </SidebarMenuItem>
{/snippet}
