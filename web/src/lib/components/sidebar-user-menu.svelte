<script lang="ts">
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import * as Avatar from '$lib/components/ui/avatar'
    import { LogOut, Settings, Plug, Brain } from '@lucide/svelte'

    interface Props {
        email: string
        isAdmin?: boolean
    }

    let { email, isAdmin = false }: Props = $props()

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }
</script>

<DropdownMenu.Root>
    <DropdownMenu.Trigger>
        {#snippet child({ props })}
            <div
                class="hover:bg-sidebar-accent hover:text-sidebar-accent-foreground flex w-full cursor-pointer items-center justify-between rounded-sm px-2 py-2 group-data-[collapsible=icon]:px-0 group-data-[collapsible=icon]:hover:bg-transparent"
                {...props}>
                <div class="flex min-w-0 flex-1 items-center gap-2.5">
                    <Avatar.Root>
                        <Avatar.Fallback class="bg-primary/80 text-primary-foreground text-xs"
                            >{email.slice(0, 2).toLocaleUpperCase()}</Avatar.Fallback>
                    </Avatar.Root>
                    <div class="flex flex-col gap-1 group-data-[collapsible=icon]:hidden">
                        <span class="truncate overflow-hidden text-sm">
                            {email}
                        </span>
                        <span class="text-muted-foreground text-xs">{isAdmin ? 'Admin' : ''}</span>
                    </div>
                </div>
            </div>
        {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content side="top" align="start" class="w-[15rem]">
        {#if isAdmin}
            <DropdownMenu.Item class="cursor-pointer">
                {#snippet child({ props })}
                    <a href="/admin/settings" {...props}>
                        <Settings class="h-4 w-4" />
                        <span>Settings</span>
                    </a>
                {/snippet}
            </DropdownMenu.Item>
            <DropdownMenu.Separator />
        {/if}
        <DropdownMenu.Item class="cursor-pointer">
            {#snippet child({ props })}
                <a href="/settings/integrations" {...props}>
                    <Plug class="h-4 w-4" />
                    <span>Integrations</span>
                </a>
            {/snippet}
        </DropdownMenu.Item>
        <DropdownMenu.Item class="cursor-pointer">
            {#snippet child({ props })}
                <a href="/settings/memory" {...props}>
                    <Brain class="h-4 w-4" />
                    <span>Memories</span>
                </a>
            {/snippet}
        </DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item onclick={logout} class="cursor-pointer">
            <LogOut class="h-4 w-4" />
            <span>Logout</span>
        </DropdownMenu.Item>
    </DropdownMenu.Content>
</DropdownMenu.Root>
