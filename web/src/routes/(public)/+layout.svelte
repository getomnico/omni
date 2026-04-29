<script lang="ts">
    import '../../app.css'
    import { themeStore } from '$lib/themes/store.svelte'
</script>

<div class="from-background to-muted min-h-screen bg-gradient-to-br">
    <div class="flex min-h-screen items-center justify-center p-4">
        <div class="w-full max-w-md">
            <div class="mb-4 flex justify-center gap-1">
                <img
                    src={themeStore.current.omniLogoLight}
                    alt="Omni logo"
                    class="omni-logo-light h-8 w-8 rounded-lg" />
                <img
                    src={themeStore.current.omniLogoDark}
                    alt="Omni logo"
                    class="omni-logo-dark h-8 w-8 rounded-lg" />
                <p class="text-2xl font-semibold">{themeStore.current.omniName}</p>
            </div>
            <slot />
        </div>
    </div>
</div>
