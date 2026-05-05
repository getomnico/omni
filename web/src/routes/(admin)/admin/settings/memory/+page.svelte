<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Select from '$lib/components/ui/select'
    import { Label } from '$lib/components/ui/label'
    import { Loader2 } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageProps } from './$types'

    let { data, form }: PageProps = $props()

    let selectedMode = $state(data.embedderAvailable ? data.orgDefault : 'off')
    let selectedLlmId = $state(data.memoryLlmId)
    let isSubmitting = $state(false)

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
                return async ({ result, update }) => {
                    await update()
                    isSubmitting = false
                    if (result.type === 'success') {
                        toast.success('Settings saved')
                    } else if (result.type === 'failure') {
                        toast.error(result.data?.error || 'Something went wrong')
                    }
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
                            }}>
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

                <div class="space-y-3">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !data.embedderAvailable}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                            Saving...
                        {:else}
                            Save settings
                        {/if}
                    </Button>
                    <p class="text-muted-foreground text-sm">
                        Users can override the memory mode in their personal preferences.
                    </p>
                </div>
            </div>
        </form>
    </div>
</div>
