<script lang="ts">
    import '../../app.css'
    import { Button } from '$lib/components/ui/button/index.js'
    import type { LayoutData } from './$types.js'
    // import omniLogo from '$lib/images/icons/omni-logo.png'

    export let data: LayoutData

    async function logout() {
        await fetch('/logout', {
            method: 'POST',
        })
        window.location.href = '/login'
    }
</script>

<!-- Header -->
<header class="bg-background sticky top-0 z-50">
    <div class="flex h-16 items-center justify-between px-6">
        <div class="flex items-center space-x-4">
            <a href="/" class="flex items-center">
                <!-- <img src={omniLogo} alt="Omni" class="h-5 w-auto" /> -->
                <span class="text-xl font-bold">omni</span>
            </a>
        </div>

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
<main class="h-[calc(100vh-4rem)]">
    <slot />
</main>
