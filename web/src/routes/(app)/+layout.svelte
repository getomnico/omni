<script lang="ts">
    import '../../app.css'
    import { Button } from '$lib/components/ui/button/index.js'
    import {
        SidebarProvider,
        Sidebar,
        SidebarContent,
        SidebarHeader,
        SidebarGroup,
        SidebarGroupContent,
        SidebarMenu,
        SidebarMenuItem,
        SidebarMenuButton,
        SidebarTrigger,
        SidebarRail,
    } from '$lib/components/ui/sidebar/index.js'
    import { Tooltip, TooltipContent, TooltipTrigger } from '$lib/components/ui/tooltip/index.js'
    import type { LayoutData } from './$types.js'
    import { MessageCirclePlus } from '@lucide/svelte'
    import type { Snippet } from 'svelte'
    import { cn } from '$lib/utils'
    import { page } from '$app/state'

    interface Props {
        data: LayoutData
        children: Snippet
    }

    let { data, children }: Props = $props()

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }
</script>

<SidebarProvider>
    <!-- Chat History Sidebar -->
    <Sidebar collapsible="icon" variant="sidebar">
        <SidebarHeader class="h-16">
            <div class="flex flex-1 items-center justify-start gap-2">
                <Tooltip>
                    <TooltipTrigger>
                        <SidebarTrigger class="cursor-pointer" />
                    </TooltipTrigger>
                    <TooltipContent>
                        <p>Toggle sidebar</p>
                    </TooltipContent>
                </Tooltip>
                <a href="/" class="flex items-center group-data-[collapsible=icon]:hidden">
                    <span class="text-xl font-bold group-data-[collapsible=icon]:hidden">omni</span>
                </a>
            </div>
        </SidebarHeader>
        <SidebarContent>
            <SidebarGroup>
                <Button
                    href="/"
                    class="my-2 flex w-full cursor-pointer items-center justify-start has-[>svg]:px-2"
                    variant="ghost">
                    <MessageCirclePlus />
                    <span class="group-data-[collapsible=icon]:hidden">New Chat</span>
                </Button>

                <SidebarGroupContent>
                    <p
                        class="text-muted-foreground mt-4 p-1.5 text-xs group-data-[collapsible=icon]:hidden">
                        Recent chats
                    </p>
                    <SidebarMenu class="gap-1.5 group-data-[collapsible=icon]:hidden">
                        {#if data.recentChats.length > 0}
                            {#each data.recentChats as chat}
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
                                </SidebarMenuItem>
                            {/each}
                        {:else}
                            <div
                                class="text-muted-foreground px-3 py-4 text-center text-sm group-data-[collapsible=icon]:hidden">
                                No chats yet
                            </div>
                        {/if}
                    </SidebarMenu>
                </SidebarGroupContent>
            </SidebarGroup>
        </SidebarContent>
        <SidebarRail />
    </Sidebar>

    <!-- Main content area -->
    <div class="flex w-full flex-col">
        <header class={cn('bg-background sticky top-0 z-50 transition-shadow')}>
            <div class="flex h-16 items-center justify-end px-6">
                <div class="flex items-center space-x-4">
                    <nav class="hidden space-x-4 md:flex">
                        {#if data.user.role === 'admin'}
                            <div class="group relative">
                                <button
                                    class="text-muted-foreground hover:text-foreground flex items-center space-x-1">
                                    <span>Admin</span>
                                    <svg
                                        class="h-4 w-4"
                                        fill="none"
                                        stroke="currentColor"
                                        viewBox="0 0 24 24">
                                        <path
                                            stroke-linecap="round"
                                            stroke-linejoin="round"
                                            stroke-width="2"
                                            d="M19 9l-7 7-7-7"></path>
                                    </svg>
                                </button>
                                <div
                                    class="bg-card border-border invisible absolute top-full right-0 z-50 mt-1 w-48 rounded-md border opacity-0 shadow-lg transition-all duration-200 group-hover:visible group-hover:opacity-100">
                                    <div class="py-1">
                                        <a
                                            href="/admin/users"
                                            class="text-foreground hover:bg-muted block px-4 py-2 text-sm">
                                            User Management
                                        </a>
                                        <a
                                            href="/admin/integrations"
                                            class="text-foreground hover:bg-muted block px-4 py-2 text-sm">
                                            Integrations
                                        </a>
                                        <a
                                            href="/admin/domains"
                                            class="text-foreground hover:bg-muted block px-4 py-2 text-sm">
                                            Domain Management
                                        </a>
                                        <a
                                            href="/admin/email-test"
                                            class="text-foreground hover:bg-muted block px-4 py-2 text-sm">
                                            Email Testing
                                        </a>
                                    </div>
                                </div>
                            </div>
                        {/if}
                    </nav>
                    <span class="text-muted-foreground text-sm">
                        {data.user.email}
                        <span class="text-muted-foreground/80 text-xs">({data.user.role})</span>
                    </span>
                    <Button variant="outline" size="sm" onclick={logout} class="cursor-pointer"
                        >Sign out</Button>
                </div>
            </div>
        </header>

        <!-- Main content -->
        <main class="flex-1">
            {@render children()}
        </main>
    </div>
</SidebarProvider>
