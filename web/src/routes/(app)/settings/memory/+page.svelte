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

    let selectedMode = $state(data.currentMode ?? '')
    let isSubmitting = $state(false)

    const orgDefaultLabel = $derived(modeLabels[data.orgDefault] ?? data.orgDefault)

    const options = $derived([
        {
            value: '',
            label: `Use org default (${orgDefaultLabel})`,
            description: 'Follow the organization-wide setting.',
        },
        {
            value: 'off',
            label: 'Off',
            description: 'Omni AI will not remember anything between sessions.',
        },
        {
            value: 'chat',
            label: 'Chat memory',
            description: 'Remember facts from your conversations.',
        },
        {
            value: 'full',
            label: 'Full memory',
            description: 'Chat memory plus agent run context.',
        },
    ])
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
                    use:enhance={() => {
                        isSubmitting = true
                        return async ({ update }) => {
                            isSubmitting = false
                            await update()
                        }
                    }}>
                    <input type="hidden" name="mode" value={selectedMode} />

                    <RadioGroup.Root
                        value={selectedMode}
                        onValueChange={(v) => {
                            selectedMode = v
                        }}
                        class="space-y-3">
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
                        <Button type="submit" disabled={isSubmitting} class="cursor-pointer">
                            {isSubmitting ? 'Saving...' : 'Save preference'}
                        </Button>
                    </div>
                </form>
            </Card.Content>
        </Card.Root>
    </div>
</div>
