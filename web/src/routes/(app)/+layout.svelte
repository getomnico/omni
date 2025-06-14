<script lang="ts">
    import '../../app.css'
    import { Button } from '$lib/components/ui/button/index.js'
    import type { LayoutData } from './$types.js'

    export let data: LayoutData

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }
</script>

<div class="bg-background min-h-screen">
    <!-- Header -->
    <header class="bg-card border-border border-b">
        <div class="flex h-16 items-center justify-between px-6">
            <div class="flex items-center space-x-4">
                <h1 class="text-foreground text-xl font-bold">Clio</h1>
                <nav class="hidden space-x-4 md:flex">
                    <a href="/" class="text-muted-foreground hover:text-foreground"> Search </a>
                    {#if data.user.role === 'admin'}
                        <a href="/admin/users" class="text-muted-foreground hover:text-foreground">
                            Admin
                        </a>
                    {/if}
                </nav>
            </div>

            <div class="flex items-center space-x-4">
                <span class="text-muted-foreground text-sm">
                    {data.user.email}
                    <span class="text-muted-foreground/80 text-xs">({data.user.role})</span>
                </span>
                <Button variant="outline" size="sm" on:click={logout}>Sign out</Button>
            </div>
        </div>
    </header>

    <!-- Main content -->
    <main class="flex-1">
        <slot />
    </main>
</div>
