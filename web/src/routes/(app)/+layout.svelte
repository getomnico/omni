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
    import {
        Tooltip,
        TooltipProvider,
        TooltipContent,
        TooltipTrigger,
    } from '$lib/components/ui/tooltip/index.js'
    import type { LayoutData } from './$types.js'
    import { LogOut, MessageCirclePlus, Settings } from '@lucide/svelte'
    import type { Snippet } from 'svelte'
    import { cn } from '$lib/utils'
    import { page } from '$app/state'
    import * as Avatar from '$lib/components/ui/avatar'

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
            <SidebarGroup>
                <div class="flex flex-col gap-1">
                    {#if data.user.role === 'admin'}
                        <div class="flex justify-start">
                            <Button
                                variant="ghost"
                                href="/admin/integrations"
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
    <div class="flex max-h-[100vh] w-full flex-col">
        <header class={cn('bg-background sticky top-0 z-50 transition-shadow')}>
            <div class="flex h-16 items-center justify-end px-6"></div>
        </header>

        <!-- Main content -->
        <main class="min-h-0 flex-1">
            {@render children()}
        </main>
    </div>
</SidebarProvider>
