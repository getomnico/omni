<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Select from '$lib/components/ui/select'
    import { Label } from '$lib/components/ui/label'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()

    let selectedMode = $state(data.embedderAvailable ? data.orgDefault : 'off')
    let selectedLlmId = $state(data.memoryLlmId)
    let isSubmitting = $state(false)
    let isDeletingAll = $state(false)

    const options = [
        {
            value: 'off',
            label: 'Off',
            description: 'Memory is disabled by default for all users.',
        },
        {
            value: 'chat',
            label: 'Chat memory',
            description: 'All users get chat memory by default.',
        },
        {
            value: 'full',
            label: 'Full memory',
            description: 'Chat memory plus agent run context for all users by default.',
        },
    ]

    let groupedModels = $derived(
        Object.entries(
            data.models.reduce<Record<string, typeof data.models>>((acc, m) => {
                if (!acc[m.providerType]) acc[m.providerType] = []
                acc[m.providerType].push(m)
                return acc
            }, {}),
        ),
    )

    let selectedModelLabel = $derived(
        selectedLlmId
            ? (data.models.find((m) => m.id === selectedLlmId)?.displayName ?? 'Unknown model')
            : 'Default model',
    )
</script>

<svelte:head>
    <title>Memory - Settings - Admin</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Memory</h1>
            <p class="text-muted-foreground mt-2">
                Set the organization-wide default memory mode. Users can override this in their
                personal preferences.
            </p>
        </div>

        {#if !data.embedderAvailable}
            <div
                class="rounded-md border border-amber-400/50 bg-amber-50/50 p-4 text-sm dark:border-amber-500/30 dark:bg-amber-950/20">
                <p class="font-medium">Memory is unavailable</p>
                <p class="text-muted-foreground mt-1">
                    Memory requires an active embedding provider. Configure one in
                    <a href="/admin/settings/embeddings" class="underline">Admin → Embeddings</a>
                    to enable memory for your organization.
                </p>
            </div>
        {/if}

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
            <input type="hidden" name="llmId" value={selectedLlmId} />

            <div class="space-y-6">
                <Card.Root>
                    <Card.Header>
                        <Card.Title>Default memory mode</Card.Title>
                        <Card.Description>
                            This setting applies to all users who have not set a personal
                            preference.
                        </Card.Description>
                    </Card.Header>
                    <Card.Content>
                        <RadioGroup.Root
                            value={selectedMode}
                            disabled={!data.embedderAvailable}
                            onValueChange={(v) => {
                                selectedMode = v
                            }}
                        >
                            {#each options as option}
                                {@const selected = selectedMode === option.value}
                                <Label
                                    for={`mode-${option.value}`}
                                    class="flex cursor-pointer items-start gap-3 rounded-md border p-4 transition-colors
                                        {selected
                                        ? 'border-blue-400/50 bg-blue-50/50 dark:border-blue-500/30 dark:bg-blue-950/20'
                                        : 'border-input hover:bg-accent/50'}">
                                    <RadioGroup.Item
                                        value={option.value}
                                        id={`mode-${option.value}`}
                                        class="mt-0.5 shrink-0" />
                                    <div>
                                        <p class="text-sm font-medium">{option.label}</p>
                                        <p class="text-muted-foreground text-sm">
                                            {option.description}
                                        </p>
                                    </div>
                                </Label>
                            {/each}
                        </RadioGroup.Root>
                    </Card.Content>
                </Card.Root>

                {#if data.models.length > 0}
                    <Card.Root>
                        <Card.Header>
                            <Card.Title>Memory model</Card.Title>
                            <Card.Description>
                                The LLM used to extract and recall memories. Must support text
                                generation. Embeddings are handled automatically by a compatible
                                installed provider.
                            </Card.Description>
                        </Card.Header>
                        <Card.Content>
                            <Select.Root
                                type="single"
                                disabled={!data.embedderAvailable}
                                value={selectedLlmId}
                                onValueChange={(v) => {
                                    selectedLlmId = v === '__default__' ? '' : v
                                }}>
                                <Select.Trigger class="w-72 cursor-pointer">
                                    {selectedModelLabel}
                                </Select.Trigger>
                                <Select.Content>
                                    <Select.Item value="__default__" class="cursor-pointer">
                                        Default model
                                    </Select.Item>
                                    <Select.Separator />
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
                        </Card.Content>
                    </Card.Root>
                {/if}

                {#if form?.error}
                    <p class="text-sm text-red-500">{form.error}</p>
                {/if}
                {#if form?.success}
                    <p class="text-muted-foreground text-sm">Settings saved.</p>
                {/if}

                <div class="space-y-3">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !data.embedderAvailable}
                        class="cursor-pointer">
                        {isSubmitting ? 'Saving...' : 'Save settings'}
                    </Button>
                    <p class="text-muted-foreground text-sm">
                        Users can override the memory mode in their personal preferences.
                    </p>
                </div>
            </div>
        </form>

        <Card.Root>
            <Card.Header>
                <Card.Title>Your stored memories</Card.Title>
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
                    <p class="text-muted-foreground text-sm">No memories stored yet.</p>
                {:else}
                    <div class="flex items-center justify-between mb-4">
                        <p class="text-muted-foreground text-sm">
                            {data.memories.length} memor{data.memories.length === 1 ? 'y' : 'ies'} stored.
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
                            <li class="flex items-start justify-between gap-4 py-3 text-sm">
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
                                    <input type="hidden" name="memoryId" value={memory.id} />
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
