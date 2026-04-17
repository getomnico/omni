<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import { Label } from '$lib/components/ui/label'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()

    const modeLabels: Record<string, string> = {
        off: 'Off',
        chat: 'Chat memory',
        full: 'Full memory',
    }

    const MODE_RANK: Record<string, number> = { off: 0, chat: 1, full: 2 }

    const orgCeiling = $derived(data.orgDefault)
    const ceilingRank = $derived(MODE_RANK[orgCeiling] ?? 0)
    const memoryLockedByOrg = $derived(orgCeiling === 'off')

    // If the user previously picked a mode above the current ceiling,
    // the saved value is still there but effectively capped. Show the
    // capped value as the initial selection.
    const initialMode = $derived(
        data.currentMode && (MODE_RANK[data.currentMode] ?? 99) > ceilingRank
            ? orgCeiling
            : (data.currentMode ?? ''),
    )

    let selectedMode = $state<string>(
        data.embedderAvailable && !memoryLockedByOrg ? initialMode : 'off',
    )
    let isSubmitting = $state(false)
    let isDeletingAll = $state(false)

    const orgDefaultLabel = $derived(modeLabels[orgCeiling] ?? orgCeiling)

    const allOptions = [
        {
            value: '',
            label: 'Use org default',
            description: 'Follow the organization-wide setting.',
            rank: -1,
        },
        {
            value: 'off',
            label: 'Off',
            description: 'Omni AI will not remember anything between sessions.',
            rank: 0,
        },
        {
            value: 'chat',
            label: 'Chat memory',
            description: 'Remember facts from your conversations.',
            rank: 1,
        },
        {
            value: 'full',
            label: 'Full memory',
            description: 'Chat memory plus agent run context.',
            rank: 2,
        },
    ]

    const options = $derived(
        allOptions
            .filter((o) => o.rank <= ceilingRank)
            .map((o) =>
                o.value === ''
                    ? { ...o, label: `Use org default (${orgDefaultLabel})` }
                    : o,
            ),
    )
</script>

<svelte:head>
    <title>Memory - Settings</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Memory</h1>
            <p class="text-muted-foreground mt-2">
                Control what Omni AI remembers about you across sessions.
            </p>
        </div>

        <Card.Root>
            <Card.Header>
                <Card.Title>Memory mode</Card.Title>
                <Card.Description>
                    Choose how much context Omni AI retains between sessions.
                </Card.Description>
            </Card.Header>
            <Card.Content>
                <form
                    method="POST"
                    action="?/save"
                    use:enhance={() => {
                        isSubmitting = true
                        return async ({ update }) => {
                            isSubmitting = false
                            await update()
                        }
                    }}>
                    <input type="hidden" name="mode" value={selectedMode} />

                    {#if !data.embedderAvailable}
                        <div
                            class="mb-4 rounded-md border border-amber-400/50 bg-amber-50/50 p-4 text-sm dark:border-amber-500/30 dark:bg-amber-950/20">
                            <p class="font-medium">Memory is unavailable</p>
                            <p class="text-muted-foreground mt-1">
                                Memory is turned off because your organization has no embedding
                                provider configured. Ask an admin to set one up to enable memory.
                            </p>
                        </div>
                    {:else if memoryLockedByOrg}
                        <div
                            class="mb-4 rounded-md border border-amber-400/50 bg-amber-50/50 p-4 text-sm dark:border-amber-500/30 dark:bg-amber-950/20">
                            <p class="font-medium">Memory is disabled by your admin</p>
                            <p class="text-muted-foreground mt-1">
                                The organization default is set to "Off", so memory is not available
                                for any user. Ask an admin to change the default if you want to use
                                memory.
                            </p>
                        </div>
                    {:else}
                        <p class="text-muted-foreground mb-4 text-sm">
                            Your admin allows up to <span class="font-medium">{orgDefaultLabel}</span
                            >. You cannot pick a higher level.
                        </p>
                    {/if}

                    <RadioGroup.Root
                        value={selectedMode}
                        disabled={!data.embedderAvailable || memoryLockedByOrg}
                        onValueChange={(v) => {
                            selectedMode = v
                        }}
                    >
                        {#each options as option}
                            {@const selected = selectedMode === option.value}
                            <Label
                                for={`mode-${option.value || 'default'}`}
                                class="flex cursor-pointer items-start gap-3 rounded-md border p-4 transition-colors
                                    {selected
                                    ? 'border-blue-400/50 bg-blue-50/50 dark:border-blue-500/30 dark:bg-blue-950/20'
                                    : 'border-input hover:bg-accent/50'}">
                                <RadioGroup.Item
                                    value={option.value}
                                    id={`mode-${option.value || 'default'}`}
                                    class="mt-0.5 shrink-0" />
                                <div>
                                    <p class="text-sm font-medium">{option.label}</p>
                                    <p class="text-muted-foreground text-sm">{option.description}</p>
                                </div>
                            </Label>
                        {/each}
                    </RadioGroup.Root>

                    {#if form?.error}
                        <p class="mt-4 text-sm text-red-500">{form.error}</p>
                    {/if}

                    {#if form?.success}
                        <p class="text-muted-foreground mt-4 text-sm">Preference saved.</p>
                    {/if}

                    <div class="mt-6">
                        <Button
                            type="submit"
                            disabled={isSubmitting ||
                                !data.embedderAvailable ||
                                memoryLockedByOrg}
                            class="cursor-pointer">
                            {isSubmitting ? 'Saving...' : 'Save preference'}
                        </Button>
                    </div>
                </form>
            </Card.Content>
        </Card.Root>

        <Card.Root>
            <Card.Header>
                <Card.Title>Stored memories</Card.Title>
                <Card.Description>
                    Everything Omni AI currently remembers about you. Delete anything you
                    do not want retained.
                </Card.Description>
            </Card.Header>
            <Card.Content>
                {#if form?.deleteError}
                    <p class="mb-4 text-sm text-red-500">{form.deleteError}</p>
                {/if}

                {#if data.memories.length === 0}
                    <p class="text-muted-foreground text-sm">
                        No memories stored yet.
                    </p>
                {:else}
                    <div class="flex items-center justify-between mb-4">
                        <p class="text-muted-foreground text-sm">
                            {data.memories.length} memor{data.memories.length === 1
                                ? 'y'
                                : 'ies'} stored.
                        </p>
                        <form
                            method="POST"
                            action="?/deleteAll"
                            use:enhance={({ cancel }) => {
                                if (
                                    !confirm(
                                        'Delete every memory Omni AI has about you? This cannot be undone.',
                                    )
                                ) {
                                    cancel()
                                    return
                                }
                                isDeletingAll = true
                                return async ({ update }) => {
                                    isDeletingAll = false
                                    await update()
                                }
                            }}>
                            <Button
                                type="submit"
                                variant="outline"
                                disabled={isDeletingAll}
                                class="cursor-pointer">
                                {isDeletingAll ? 'Deleting...' : 'Delete all'}
                            </Button>
                        </form>
                    </div>

                    <ul class="divide-y border-t border-b">
                        {#each data.memories as memory (memory.id)}
                            <li
                                class="flex items-start justify-between gap-4 py-3 text-sm">
                                <span class="flex-1 whitespace-pre-wrap break-words">
                                    {memory.memory}
                                </span>
                                <form
                                    method="POST"
                                    action="?/deleteOne"
                                    use:enhance={() => {
                                        return async ({ update }) => {
                                            await update()
                                        }
                                    }}>
                                    <input
                                        type="hidden"
                                        name="memoryId"
                                        value={memory.id} />
                                    <Button
                                        type="submit"
                                        variant="ghost"
                                        size="sm"
                                        class="cursor-pointer">
                                        Delete
                                    </Button>
                                </form>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </Card.Content>
        </Card.Root>
    </div>
</div>
