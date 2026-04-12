<script lang="ts">
    import { enhance } from '$app/forms'
    import * as Card from '$lib/components/ui/card'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import { Badge } from '$lib/components/ui/badge'
    import { Label } from '$lib/components/ui/label'
    import { Separator } from '$lib/components/ui/separator'
    import { Switch } from '$lib/components/ui/switch'
    import { Sparkles, AlertTriangle } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'

    let { data }: { data: PageData } = $props()

    let doclingEnabled = $state(data.doclingEnabled)
    let qualityPreset = $state(data.qualityPreset)
    let isSubmitting = $state(false)
    let isPresetSubmitting = $state(false)
    let enableFormRef = $state<HTMLFormElement | null>(null)
    let presetFormRef = $state<HTMLFormElement | null>(null)

    const presets = [
        {
            value: 'fast',
            label: 'Fast',
            description: 'Text-heavy docs. Basic tables.',
        },
        {
            value: 'balanced',
            label: 'Balanced',
            description: 'Accurate tables + image classification.',
            isDefault: true,
        },
        {
            value: 'quality',
            label: 'Quality',
            description: 'High-res with table and figure images.',
        },
    ]

    const presetLabels: Record<string, string> = Object.fromEntries(
        presets.map((p) => [p.value, p.label]),
    )
</script>

<svelte:head>
    <title>Document Conversion - Settings - Omni</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Document conversion</h1>
            <p class="text-muted-foreground mt-1 text-sm">
                How uploaded files are parsed into searchable text.
            </p>
        </div>

        <Card.Root>
            <Card.Header>
                <div class="flex items-start gap-3">
                    <div
                        class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-purple-100 dark:bg-purple-950">
                        <Sparkles class="h-[18px] w-[18px] text-purple-700 dark:text-purple-300" />
                    </div>
                    <div class="min-w-0 flex-1">
                        <div class="flex items-center justify-between gap-3">
                            <div>
                                <p class="text-base font-medium">AI-powered extraction</p>
                                <p class="text-muted-foreground mt-0.5 text-sm">
                                    Docling &middot; layout-aware OCR for PDFs, Office files, and
                                    images
                                </p>
                            </div>
                            <form
                                method="POST"
                                action="?/updateDocling"
                                bind:this={enableFormRef}
                                use:enhance={({ formData }) => {
                                    if (doclingEnabled) {
                                        formData.set('enabled', 'true')
                                    } else {
                                        formData.delete('enabled')
                                    }
                                    isSubmitting = true
                                    return async ({ result, update }) => {
                                        isSubmitting = false
                                        await update()
                                        if (result.type === 'success') {
                                            toast.success(result.data?.message || 'Setting updated')
                                        } else if (result.type === 'failure') {
                                            toast.error(
                                                result.data?.error || 'Something went wrong',
                                            )
                                            doclingEnabled = data.doclingEnabled
                                        }
                                    }
                                }}>
                                <Switch
                                    name="enabled"
                                    value="true"
                                    checked={doclingEnabled}
                                    disabled={isSubmitting}
                                    onCheckedChange={(checked) => {
                                        doclingEnabled = checked
                                        enableFormRef?.requestSubmit()
                                    }}
                                    class="cursor-pointer" />
                            </form>
                        </div>

                        {#if data.doclingReachable}
                            <div
                                class="mt-2.5 inline-flex items-center gap-1.5 text-sm text-green-600 dark:text-green-400">
                                <span class="h-1.5 w-1.5 rounded-full bg-current"></span>
                                Service healthy
                            </div>
                        {:else}
                            <div
                                class="mt-2.5 inline-flex items-center gap-1.5 text-sm text-red-600 dark:text-red-400">
                                <AlertTriangle class="h-3.5 w-3.5" />
                                Service unreachable &mdash; check
                                <code class="bg-muted rounded px-1.5 py-0.5 text-xs"
                                    >docker compose logs docling</code>
                            </div>
                        {/if}
                    </div>
                </div>
            </Card.Header>

            {#if doclingEnabled}
                <Card.Content>
                    <Separator class="mb-5" />

                    <div class="mb-3 flex items-baseline justify-between">
                        <Label class="text-sm font-medium">Extraction quality</Label>
                        <span class="text-muted-foreground text-sm">Quality vs. speed</span>
                    </div>

                    <form
                        method="POST"
                        action="?/updateQualityPreset"
                        bind:this={presetFormRef}
                        use:enhance={({ formData }) => {
                            formData.set('preset', qualityPreset)
                            isPresetSubmitting = true
                            return async ({ result, update }) => {
                                isPresetSubmitting = false
                                await update()
                                if (result.type === 'success') {
                                    toast.success(
                                        `Quality preset updated to "${presetLabels[qualityPreset] ?? qualityPreset}"`,
                                    )
                                } else if (result.type === 'failure') {
                                    toast.error(result.data?.error || 'Something went wrong')
                                    qualityPreset = data.qualityPreset
                                }
                            }
                        }}>
                        <input type="hidden" name="preset" value={qualityPreset} />
                        <RadioGroup.Root
                            bind:value={qualityPreset}
                            disabled={isPresetSubmitting}
                            onValueChange={(value) => {
                                qualityPreset = value
                                presetFormRef?.requestSubmit()
                            }}
                            class="grid grid-cols-3 gap-2">
                            {#each presets as preset}
                                {@const selected = qualityPreset === preset.value}
                                <Label
                                    for={preset.value}
                                    class="relative flex cursor-pointer flex-col items-start rounded-md border p-4 transition-colors
                                        {selected
                                        ? 'border-blue-400/50 bg-blue-50/50 dark:border-blue-500/30 dark:bg-blue-950/20'
                                        : 'border-input hover:bg-accent/50'}">
                                    <RadioGroup.Item
                                        value={preset.value}
                                        id={preset.value}
                                        class="sr-only" />
                                    {#if preset.isDefault}
                                        <Badge
                                            variant="secondary"
                                            class="absolute top-2 right-2 text-xs">
                                            Default
                                        </Badge>
                                    {/if}
                                    <div class="mb-1">
                                        <span class="text-sm font-medium">
                                            {preset.label}
                                        </span>
                                    </div>
                                    <p class="text-muted-foreground text-sm leading-relaxed">
                                        {preset.description}
                                    </p>
                                </Label>
                            {/each}
                        </RadioGroup.Root>
                    </form>
                </Card.Content>
            {/if}
        </Card.Root>
    </div>
</div>
