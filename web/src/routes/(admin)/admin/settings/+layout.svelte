<script lang="ts">
    import * as Sidebar from '$lib/components/ui/sidebar'
    import type { Snippet } from 'svelte'
    import { cn } from '$lib/utils'
    import { page } from '$app/state'
    import { ArrowLeft, Cable, Users, Brain, ArrowUpRight } from '@lucide/svelte'
    import Button from '$lib/components/ui/button/button.svelte'

    interface Props {
        children: Snippet
    }

    let { children }: Props = $props()
</script>

<Sidebar.Provider>
    <Sidebar.Root variant="floating" collapsible="none" class="h-svh border-r">
        <Sidebar.Header class="flex justify-start">
            <Button
                variant="ghost"
                href="/"
                class="text-muted-foreground flex w-fit cursor-pointer justify-start text-sm">
                <ArrowLeft class="h-4 w-4" />
                Back
            </Button>
        </Sidebar.Header>
        <Sidebar.Content>
            <Sidebar.Group>
                <Sidebar.GroupLabel>Account</Sidebar.GroupLabel>
                <Sidebar.GroupContent>
                    <Sidebar.Menu>
                        <Sidebar.MenuItem>
                            <Sidebar.MenuButton
                                class={cn(
                                    page.url.pathname === '/admin/settings/integrations' &&
                                        'bg-sidebar-accent text-sidebar-accent-foreground',
                                )}>
                                {#snippet child({ props })}
                                    <a href="/admin/settings/integrations" {...props}>
                                        <Cable class="h-4 w-4" />
                                        <span>Integrations</span>
                                    </a>
                                {/snippet}
                            </Sidebar.MenuButton>
                        </Sidebar.MenuItem>
                        <Sidebar.MenuItem>
                            <Sidebar.MenuButton
                                class={cn(
                                    page.url.pathname === '/admin/settings/user-management' &&
                                        'bg-sidebar-accent text-sidebar-accent-foreground',
                                )}>
                                {#snippet child({ props })}
                                    <a href="/admin/settings/user-management" {...props}>
                                        <Users class="h-4 w-4" />
                                        <span>User Management</span>
                                    </a>
                                {/snippet}
                            </Sidebar.MenuButton>
                        </Sidebar.MenuItem>
                        <Sidebar.MenuItem>
                            <Sidebar.MenuButton
                                class={cn(
                                    page.url.pathname === '/admin/settings/llm' &&
                                        'bg-sidebar-accent text-sidebar-accent-foreground',
                                )}>
                                {#snippet child({ props })}
                                    <a href="/admin/settings/llm" {...props}>
                                        <Brain class="h-4 w-4" />
                                        <span>LLM Config</span>
                                    </a>
                                {/snippet}
                            </Sidebar.MenuButton>
                        </Sidebar.MenuItem>
                        <Sidebar.MenuItem>
                            <Sidebar.MenuButton
                                class={cn(
                                    page.url.pathname === '/admin/settings/embeddings' &&
                                        'bg-sidebar-accent text-sidebar-accent-foreground',
                                )}>
                                {#snippet child({ props })}
                                    <a href="/admin/settings/embeddings" {...props}>
                                        <ArrowUpRight class="h-4 w-4" />
                                        <span>Embedding Config</span>
                                    </a>
                                {/snippet}
                            </Sidebar.MenuButton>
                        </Sidebar.MenuItem>
                    </Sidebar.Menu>
                </Sidebar.GroupContent>
            </Sidebar.Group>
        </Sidebar.Content>
        <Sidebar.Rail />
    </Sidebar.Root>

    <!-- Main content area -->
    <div class="flex max-h-[100svh] min-h-screen w-full flex-col">
        <main class="min-h-0 flex-1">
            {@render children()}
        </main>
    </div>
</Sidebar.Provider>
