<script lang="ts">
    import { enhance } from '$app/forms'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import * as RadioGroup from '$lib/components/ui/radio-group'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import { Sparkles, AlertTriangle, CircleCheck, Zap, Scale, Gem } from '@lucide/svelte'
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
            description:
                'Fastest extraction. Basic table detection, standard resolution. Best for text-heavy documents.',
            icon: Zap,
        },
        {
            value: 'balanced',
            label: 'Balanced',
            description:
                'Good quality for most documents. Accurate table structure with image classification.',
            icon: Scale,
        },
        {
            value: 'quality',
            label: 'Quality',
            description:
                'Best extraction quality. High-resolution processing, generates table and picture images.',
            icon: Gem,
        },
    ]
</script>

<svelte:head>
    <title>Document Conversion - Settings - Omni</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Document Conversion</h1>
            <p class="text-muted-foreground mt-2">
                Configure how documents are converted to text for indexing
            </p>
        </div>

        <Card.Root>
            <Card.Header>
                <div class="flex items-center gap-3">
                    <div
                        class="flex h-10 w-10 items-center justify-center rounded-lg bg-gradient-to-br from-purple-500 to-indigo-600">
                        <Sparkles class="h-5 w-5 text-white" />
                    </div>
                    <div>
                        <div class="text-base leading-tight font-semibold">
                            AI-Powered Document Conversion
                        </div>
                        <p class="text-muted-foreground mt-0.5 text-sm">Powered by Docling</p>
                    </div>
                </div>
                <Card.Action>
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
                                    toast.error(result.data?.error || 'Something went wrong')
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
                </Card.Action>
            </Card.Header>
            <Card.Content>
                <p class="text-muted-foreground mb-4 text-sm">
                    Uses AI-based layout analysis with built-in OCR to extract text from PDFs,
                    Office documents, and images. Produces structure-aware Markdown that preserves
                    tables, headings, and reading order for higher-quality search results.
                </p>

                {#if data.doclingReachable}
                    <Alert.Root variant="default">
                        <CircleCheck class="h-4 w-4" />
                        <Alert.Title>Service healthy</Alert.Title>
                        <Alert.Description>
                            The Docling service is running and ready to process documents.
                        </Alert.Description>
                    </Alert.Root>
                {:else}
                    <Alert.Root variant="destructive">
                        <AlertTriangle class="h-4 w-4" />
                        <Alert.Title>Service unreachable</Alert.Title>
                        <Alert.Description>
                            The Docling service is not responding. It may still be loading models
                            after a fresh start. Check the service logs:
                            <code class="bg-muted mt-1 block rounded px-2 py-1 text-sm">
                                docker compose logs docling
                            </code>
                        </Alert.Description>
                    </Alert.Root>
                {/if}
            </Card.Content>
        </Card.Root>

        {#if doclingEnabled}
            <Card.Root>
                <Card.Header>
                    <div>
                        <div class="text-base leading-tight font-semibold">Extraction Quality</div>
                        <p class="text-muted-foreground mt-0.5 text-sm">
                            Trade off between extraction quality and processing speed
                        </p>
                    </div>
                </Card.Header>
                <Card.Content>
                    <form
                        method="POST"
                        action="?/updateQualityPreset"
                        bind:this={presetFormRef}
                        use:enhance={() => {
                            isPresetSubmitting = true
                            return async ({ result, update }) => {
                                isPresetSubmitting = false
                                await update()
                                if (result.type === 'success') {
                                    toast.success(result.data?.message || 'Preset updated')
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
                            onValueChange={() => {
                                presetFormRef?.requestSubmit()
                            }}
                            class="grid gap-3">
                            {#each presets as preset}
                                <Label
                                    for={preset.value}
                                    class="border-input hover:bg-accent/50 flex cursor-pointer items-start gap-3 rounded-lg border p-4 transition-colors has-[data-state=checked]:border-purple-500/50 has-[data-state=checked]:bg-purple-500/5">
                                    <RadioGroup.Item
                                        value={preset.value}
                                        id={preset.value}
                                        class="mt-0.5" />
                                    <div class="flex items-start gap-3">
                                        <preset.icon
                                            class="text-muted-foreground mt-0.5 h-4 w-4 shrink-0" />
                                        <div>
                                            <div class="text-sm font-medium">{preset.label}</div>
                                            <div class="text-muted-foreground mt-0.5 text-xs">
                                                {preset.description}
                                            </div>
                                        </div>
                                    </div>
                                </Label>
                            {/each}
                        </RadioGroup.Root>
                    </form>
                </Card.Content>
            </Card.Root>
        {/if}
    </div>
</div>
