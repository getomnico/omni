<script lang="ts">
    import { enhance } from '$app/forms'
    import { beforeNavigate } from '$app/navigation'
    import { onMount } from 'svelte'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import * as Select from '$lib/components/ui/select'
    import { Separator } from '$lib/components/ui/separator'
    import { Label } from '$lib/components/ui/label'
    import { AlertTriangle, Loader2, MessageSquare, PowerOff, Sparkles } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import { formatProviderName } from '$lib/utils/providers.js'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()

    type Mode = 'off' | 'chat' | 'full'

    const initialMode: Mode = (data.embedderAvailable ? data.orgDefault : 'off') as Mode

    let selectedMode = $state<Mode>(initialMode)
    let selectedLlmId = $state(data.memoryLlmId)
    let savedMode = $state<Mode>(initialMode)
    let savedLlmId = $state(data.memoryLlmId)
    let isSubmitting = $state(false)
    let skipUnsavedCheck = $state(false)

    let hasUnsavedChanges = $derived(selectedMode !== savedMode || selectedLlmId !== savedLlmId)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null
    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }
        window.addEventListener('beforeunload', beforeUnloadHandler)
        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
                beforeUnloadHandler = null
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const ok = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!ok) cancel()
        }
    })

    const modeOptions = [
        {
            value: 'off' as Mode,
            label: 'Off',
            description: 'Memory is disabled by default for all users.',
            icon: PowerOff,
            accent: 'text-muted-foreground',
        },
        {
            value: 'chat' as Mode,
            label: 'Chat memory',
            description: 'All users get chat memory by default.',
            icon: MessageSquare,
            accent: 'text-blue-600 dark:text-blue-400',
        },
        {
            value: 'full' as Mode,
            label: 'Full memory',
            description: 'Chat memory plus agent run context for all users by default.',
            icon: Sparkles,
            accent: 'text-indigo-600 dark:text-indigo-400',
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

    let savedModelLabel = $derived(
        savedLlmId
            ? (data.models.find((m) => m.id === savedLlmId)?.displayName ?? 'Unknown model')
            : 'Default model',
    )
    let selectedModelLabel = $derived(
        selectedLlmId
            ? (data.models.find((m) => m.id === selectedLlmId)?.displayName ?? 'Unknown model')
            : 'Default model',
    )

    let selectedModeOption = $derived(
        modeOptions.find((o) => o.value === selectedMode) ?? modeOptions[0],
    )
    let savedModeOption = $derived(modeOptions.find((o) => o.value === savedMode) ?? modeOptions[0])
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

        <form
            method="POST"
            action="?/save"
            use:enhance={() => {
                isSubmitting = true
                return async ({ result, update }) => {
                    await update()
                    isSubmitting = false
                    if (result.type === 'success') {
                        savedMode = selectedMode
                        savedLlmId = selectedLlmId
                        toast.success('Settings saved')
                    } else if (result.type === 'failure') {
                        toast.error(
                            (result.data?.error as string | undefined) || 'Something went wrong',
                        )
                    }
                }
            }}>
            <input type="hidden" name="mode" value={selectedMode} />
            <input type="hidden" name="llmId" value={selectedLlmId} />

            <Card.Root>
                <Card.Content class="space-y-6">
                    {#if !data.embedderAvailable}
                        <div class="flex items-start gap-4">
                            <div
                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-amber-300/70 bg-amber-50 shadow-sm dark:border-amber-500/40 dark:bg-amber-950/30">
                                <AlertTriangle class="h-5 w-5 text-amber-600 dark:text-amber-400" />
                            </div>
                            <div class="min-w-0">
                                <p class="text-muted-foreground text-xs tracking-wide uppercase">
                                    Status
                                </p>
                                <p class="text-sm font-medium">Memory unavailable</p>
                                <p class="text-muted-foreground mt-1 text-sm">
                                    Memory requires an active embedding provider. Configure one in
                                    <a
                                        href="/admin/settings/embeddings"
                                        class="underline underline-offset-2">Admin → Embeddings</a>
                                    to enable memory for your organization.
                                </p>
                            </div>
                        </div>
                    {:else}
                        {@const Icon = savedModeOption.icon}
                        <div class="flex items-start gap-4">
                            <div
                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm dark:border-slate-700/60 dark:bg-slate-900/40">
                                <Icon class="h-5 w-5 {savedModeOption.accent}" />
                            </div>
                            <div class="min-w-0">
                                <p class="text-sm font-medium">
                                    {savedModeOption.label}
                                    {#if savedMode !== 'off'}
                                        <span class="text-muted-foreground">·</span>
                                        {savedModelLabel}
                                    {/if}
                                </p>
                                <p class="text-muted-foreground mt-1 text-sm">
                                    {savedModeOption.description}
                                </p>
                            </div>
                        </div>
                    {/if}

                    <Separator />

                    <div
                        class="grid px-2 md:grid-cols-[16rem_1fr] [&>*:last-child]:justify-self-end">
                        <div>
                            <p class="text-sm font-medium">Default mode</p>
                            <p class="text-muted-foreground mt-1 text-sm">
                                Applies to all users who have not set a personal preference.
                            </p>
                        </div>
                        <div class="flex flex-col">
                            <RadioGroup.Root
                                value={selectedMode}
                                disabled={!data.embedderAvailable}
                                onValueChange={(v) => {
                                    selectedMode = v as Mode
                                }}
                                class="bg-muted/60 grid grid-cols-3 gap-1 rounded-lg border p-1">
                                {#each modeOptions as option}
                                    {@const selected = selectedMode === option.value}
                                    <Label
                                        for={`mode-${option.value}`}
                                        class="relative flex cursor-pointer items-center justify-center gap-2 rounded-md px-2 py-2 text-sm font-medium transition-colors
                                            {selected
                                            ? 'bg-background text-foreground shadow-sm'
                                            : 'text-muted-foreground hover:text-foreground'}
                                            {!data.embedderAvailable
                                            ? 'cursor-not-allowed opacity-60'
                                            : ''}">
                                        <RadioGroup.Item
                                            value={option.value}
                                            id={`mode-${option.value}`}
                                            class="sr-only" />
                                        <option.icon class="h-4 w-4" />
                                        {option.label}
                                    </Label>
                                {/each}
                            </RadioGroup.Root>
                            <p class="text-muted-foreground mt-3 self-end text-sm">
                                {selectedModeOption.description}
                            </p>
                        </div>
                    </div>

                    {#if data.models.length > 0 && selectedMode !== 'off'}
                        <Separator />

                        <div class="grid gap-4 px-2 md:grid-cols-[16rem_1fr]">
                            <div>
                                <p class="text-sm font-medium">Memory model</p>
                                <p class="text-muted-foreground mt-1 text-sm">
                                    LLM used to extract and recall memories. Embeddings are handled
                                    automatically by a compatible installed provider.
                                </p>
                            </div>
                            <div class="justify-self-end">
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
                            </div>
                        </div>
                    {/if}
                </Card.Content>
                <Card.Footer class="flex justify-end border-t">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !data.embedderAvailable || !hasUnsavedChanges}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                            Saving...
                        {:else}
                            Save settings
                        {/if}
                    </Button>
                </Card.Footer>
            </Card.Root>
        </form>
    </div>
</div>
