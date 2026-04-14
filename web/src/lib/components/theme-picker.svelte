<script lang="ts">
    import Palette from '@lucide/svelte/icons/palette'
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
</script>

<TooltipProvider delayDuration={300}>
    <Tooltip>
        <TooltipTrigger>
            <Button
                variant="ghost"
                size="icon"
                title="Switch theme"
                aria-label="Switch theme"
                class="cursor-pointer {className}"
                onclick={() => {
                    const idx = themes.findIndex((t) => t.id === themeStore.current.id)
                    themeStore.set(themes[(idx + 1) % themes.length].id)
                }}
            >
                <Palette class="h-4 w-4" />
            </Button>
        </TooltipTrigger>
        <TooltipContent>
            <p>Switch theme</p>
        </TooltipContent>
    </Tooltip>
</TooltipProvider>
