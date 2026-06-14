<script lang="ts">
    import Check from '@lucide/svelte/icons/check'
    import Monitor from '@lucide/svelte/icons/monitor'
    import Moon from '@lucide/svelte/icons/moon'
    import Sun from '@lucide/svelte/icons/sun'
    import { Button } from '$lib/components/ui/button'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import { themeStore } from '$lib/themes/store.svelte'
    import type { ThemePreference } from '$lib/preferences/user-preferences'

    let { class: className = '' }: { class?: string } = $props()

    const themeOptions: Array<{
        value: ThemePreference
        label: string
        icon: typeof Sun
    }> = [
        { value: 'light', label: 'Light', icon: Sun },
        { value: 'dark', label: 'Dark', icon: Moon },
        { value: 'system', label: 'System', icon: Monitor },
    ]

    function currentThemeOption() {
        return (
            themeOptions.find((option) => option.value === themeStore.preference) ?? themeOptions[0]
        )
    }
</script>

<DropdownMenu.Root>
    <DropdownMenu.Trigger>
        {#snippet child({ props })}
            <Button
                variant="ghost"
                size="icon"
                title="Choose theme"
                aria-label="Choose theme"
                class="cursor-pointer {className}"
                {...props}>
                {@const CurrentIcon = currentThemeOption().icon}
                <CurrentIcon class="h-4 w-4" />
            </Button>
        {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end" class="w-40">
        {#each themeOptions as option (option.value)}
            <DropdownMenu.Item class="cursor-pointer" onclick={() => themeStore.set(option.value)}>
                {@const Icon = option.icon}
                <Icon class="h-4 w-4" />
                <span>{option.label}</span>
                {#if themeStore.preference === option.value}
                    <Check class="ml-auto h-4 w-4" />
                {/if}
            </DropdownMenu.Item>
        {/each}
    </DropdownMenu.Content>
</DropdownMenu.Root>
