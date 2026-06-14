<script lang="ts">
    import Moon from '@lucide/svelte/icons/moon'
    import Sun from '@lucide/svelte/icons/sun'
    import { Button } from '$lib/components/ui/button'
    import { themeStore } from '$lib/themes/store.svelte'
    import { themes } from '$lib/themes/registry'
    import {
        Tooltip,
        TooltipProvider,
        TooltipContent,
        TooltipTrigger,
    } from '$lib/components/ui/tooltip'

    let { class: className = '' }: { class?: string } = $props()

    function nextTheme() {
        const idx = themes.findIndex((t) => t.id === themeStore.current.id)
        return themes[(idx + 1) % themes.length]
    }
</script>

<TooltipProvider delayDuration={300}>
    <Tooltip>
        <TooltipTrigger>
            <Button
                variant="ghost"
                size="icon"
                title={`Switch to ${nextTheme().name} theme`}
                aria-label={`Switch to ${nextTheme().name} theme`}
                class="cursor-pointer {className}"
                onclick={() => themeStore.set(nextTheme().id)}>
                {#if themeStore.current.id === 'dark'}
                    <Sun class="h-4 w-4" />
                {:else}
                    <Moon class="h-4 w-4" />
                {/if}
            </Button>
        </TooltipTrigger>
        <TooltipContent>
            <p>Switch to {nextTheme().name} theme</p>
        </TooltipContent>
    </Tooltip>
</TooltipProvider>
