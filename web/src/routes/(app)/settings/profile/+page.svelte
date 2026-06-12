<script lang="ts">
    import { browser } from '$app/environment'
    import { enhance } from '$app/forms'
    import { invalidateAll } from '$app/navigation'
    import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down'
    import { tick } from 'svelte'
    import ThemePicker from '$lib/components/theme-picker.svelte'
    import { Badge } from '$lib/components/ui/badge'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as Command from '$lib/components/ui/command/index.js'
    import { Label } from '$lib/components/ui/label'
    import * as Popover from '$lib/components/ui/popover/index.js'
    import * as Select from '$lib/components/ui/select/index.js'
    import { userPreferences } from '$lib/preferences'
    import type { InputMode } from '$lib/components/user-input.svelte'
    import { themeStore } from '$lib/themes/store.svelte'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()

    function timezoneOptions(): string[] {
        const supportedValuesOf = (
            Intl as unknown as {
                supportedValuesOf?: (key: 'timeZone') => string[]
            }
        ).supportedValuesOf
        const zones = supportedValuesOf ? supportedValuesOf('timeZone') : []
        const values = new Set<string>(zones)
        values.add(data.timezone)

        const detected = browser ? Intl.DateTimeFormat().resolvedOptions().timeZone : null
        if (detected) values.add(detected)

        return Array.from(values).sort((a, b) => a.localeCompare(b))
    }

    let timezone = $state(data.timezone)
    let timezoneOpen = $state(false)
    let timezoneTriggerRef = $state<HTMLButtonElement>(null!)
    let inputMode = $state<InputMode>(userPreferences.get('inputMode'))
    let preferredModelId = $state<string>(userPreferences.get('preferredModelId') ?? '')
    let isSubmitting = $state(false)
    const timezones = $derived(timezoneOptions())
    const defaultModel = $derived(data.models.find((model) => model.isDefault))
    const selectedTimezoneLabel = $derived(timezone || 'Select a timezone...')
    const groupedModels = $derived(
        Object.entries(
            data.models.reduce<Record<string, typeof data.models>>((groups, model) => {
                const provider = model.providerType
                groups[provider] ??= []
                groups[provider].push(model)
                return groups
            }, {}),
        ),
    )
    const preferredModel = $derived(
        preferredModelId ? data.models.find((model) => model.id === preferredModelId) : null,
    )
    const selectedModelLabel = $derived(
        preferredModel?.displayName ?? defaultModel?.displayName ?? 'System default',
    )
    const currentThemeLabel = $derived(themeStore.current.colorScheme === 'dark' ? 'Dark' : 'Light')

    function closeTimezoneAndFocusTrigger() {
        timezoneOpen = false
        tick().then(() => timezoneTriggerRef.focus())
    }

    function saveInputMode(value: string) {
        inputMode = value as InputMode
        userPreferences.set('inputMode', inputMode)
    }

    function savePreferredModel(value: string) {
        preferredModelId = value
        userPreferences.set('preferredModelId', value || null)
    }
</script>

<svelte:head>
    <title>Profile - Settings</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-md space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Profile</h1>
            <p class="text-muted-foreground mt-2">Manage your account and personal preferences.</p>
        </div>

        <Card.Root>
            <Card.Header>
                <Card.Title>Account</Card.Title>
                <Card.Description>Your Omni account details.</Card.Description>
            </Card.Header>
            <Card.Content class="space-y-4">
                <div class="grid gap-4 sm:grid-cols-2">
                    <div class="space-y-1">
                        <p class="text-muted-foreground text-sm">Email</p>
                        <p class="font-medium">{data.user.email}</p>
                    </div>
                    <div class="space-y-1">
                        <p class="text-muted-foreground text-sm">Role</p>
                        <Badge variant="secondary" class="capitalize">{data.user.role}</Badge>
                    </div>
                </div>

                {#if data.canChangePassword}
                    <Button href="/change-password" variant="outline" class="cursor-pointer">
                        Change password
                    </Button>
                {/if}
            </Card.Content>
        </Card.Root>

        <Card.Root>
            <Card.Header>
                <Card.Title>Preferences</Card.Title>
                <Card.Description>
                    These settings control how Omni looks and behaves for you.
                </Card.Description>
            </Card.Header>
            <Card.Content class="space-y-6">
                <form
                    method="POST"
                    action="?/saveTimezone"
                    class="space-y-4"
                    use:enhance={() => {
                        isSubmitting = true
                        return async ({ update }) => {
                            isSubmitting = false
                            await update()
                            await invalidateAll()
                        }
                    }}>
                    <div class="space-y-2">
                        <Label for="timezone">Timezone</Label>
                        <input type="hidden" name="timezone" value={timezone} />
                        <Popover.Root bind:open={timezoneOpen}>
                            <Popover.Trigger bind:ref={timezoneTriggerRef}>
                                {#snippet child({ props })}
                                    <Button
                                        {...props}
                                        id="timezone"
                                        variant="outline"
                                        class="w-full justify-between sm:w-[320px]"
                                        role="combobox"
                                        aria-expanded={timezoneOpen}>
                                        <span class="truncate">{selectedTimezoneLabel}</span>
                                        <ChevronsUpDownIcon class="opacity-50" />
                                    </Button>
                                {/snippet}
                            </Popover.Trigger>
                            <Popover.Content class="w-[320px] p-0" align="start">
                                <Command.Root bind:value={timezone}>
                                    <Command.Input placeholder="Search timezones..." />
                                    <Command.List>
                                        <Command.Empty>No timezone found.</Command.Empty>
                                        <Command.Group value="timezones">
                                            {#each timezones as option (option)}
                                                <Command.Item
                                                    value={option}
                                                    onSelect={() => {
                                                        timezone = option
                                                        closeTimezoneAndFocusTrigger()
                                                    }}>
                                                    {option}
                                                </Command.Item>
                                            {/each}
                                        </Command.Group>
                                    </Command.List>
                                </Command.Root>
                            </Popover.Content>
                        </Popover.Root>
                    </div>

                    {#if !data.timezoneSaved}
                        <p class="text-muted-foreground text-sm">
                            Your browser timezone was not saved yet. Omni is currently using UTC as
                            a fallback until a timezone is saved.
                        </p>
                    {/if}

                    {#if form?.error}
                        <p class="text-sm text-red-500">{form.error}</p>
                    {/if}

                    {#if form?.success}
                        <p class="text-muted-foreground text-sm">Timezone saved.</p>
                    {/if}

                    <Button type="submit" disabled={isSubmitting} class="cursor-pointer">
                        {isSubmitting ? 'Saving...' : 'Save timezone'}
                    </Button>
                </form>

                <div class="border-border border-t pt-6">
                    <div class="flex items-center justify-between gap-4">
                        <div>
                            <Label for="input-mode">Default input mode</Label>
                            <p class="text-muted-foreground mt-1 text-sm">
                                Choose whether the home input starts in chat or search mode.
                            </p>
                        </div>
                        <Select.Root
                            type="single"
                            value={inputMode}
                            onValueChange={(value) => saveInputMode(value)}>
                            <Select.Trigger id="input-mode" class="w-36 cursor-pointer">
                                {inputMode === 'chat' ? 'Chat' : 'Search'}
                            </Select.Trigger>
                            <Select.Content>
                                <Select.Item value="chat" class="cursor-pointer">Chat</Select.Item>
                                <Select.Item value="search" class="cursor-pointer"
                                    >Search</Select.Item>
                            </Select.Content>
                        </Select.Root>
                    </div>
                </div>

                {#if data.models.length > 1}
                    <div class="border-border border-t pt-6">
                        <div
                            class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
                            <div>
                                <Label for="preferred-model">Preferred chat model</Label>
                            </div>
                            <Select.Root
                                type="single"
                                value={preferredModelId || 'system-default'}
                                onValueChange={(value) =>
                                    savePreferredModel(value === 'system-default' ? '' : value)}>
                                <Select.Trigger
                                    id="preferred-model"
                                    class="w-full cursor-pointer sm:w-72">
                                    {selectedModelLabel}
                                </Select.Trigger>
                                <Select.Content>
                                    <Select.Item value="system-default" class="cursor-pointer">
                                        System default
                                    </Select.Item>
                                    {#each groupedModels as [provider, providerModels]}
                                        <Select.Group>
                                            <Select.GroupHeading>
                                                {formatProviderName(provider)}
                                            </Select.GroupHeading>
                                            {#each providerModels as model}
                                                <Select.Item
                                                    value={model.id}
                                                    class="cursor-pointer">
                                                    {model.displayName}
                                                </Select.Item>
                                            {/each}
                                        </Select.Group>
                                    {/each}
                                </Select.Content>
                            </Select.Root>
                        </div>
                    </div>
                {/if}

                <div class="border-border border-t pt-6">
                    <div class="flex items-center justify-between gap-4">
                        <div>
                            <p class="text-sm font-medium">Appearance</p>
                            <p class="text-muted-foreground mt-1 text-sm">
                                Current theme: {currentThemeLabel}
                            </p>
                        </div>
                        <ThemePicker />
                    </div>
                </div>
            </Card.Content>
        </Card.Root>

        <Card.Root>
            <Card.Header>
                <Card.Title>Quick links</Card.Title>
                <Card.Description>Jump to other personal settings.</Card.Description>
            </Card.Header>
            <Card.Content class="flex flex-wrap gap-3">
                <Button href="/settings/integrations" variant="outline" class="cursor-pointer">
                    My integrations
                </Button>
                {#if data.memoryEnabled}
                    <Button href="/settings/memory" variant="outline" class="cursor-pointer">
                        Memory
                    </Button>
                {/if}
                {#if data.user.role === 'admin'}
                    <Button href="/admin/settings" variant="outline" class="cursor-pointer">
                        Admin settings
                    </Button>
                {/if}
            </Card.Content>
        </Card.Root>
    </div>
</div>
