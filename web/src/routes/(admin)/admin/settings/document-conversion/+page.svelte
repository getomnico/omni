<script lang="ts">
    import { enhance } from '$app/forms'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Switch } from '$lib/components/ui/switch'
    import { Info, Sparkles, AlertTriangle, CircleCheck } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageData } from './$types'

    let { data }: { data: PageData } = $props()

    let doclingEnabled = $state(data.doclingEnabled)
    let isSubmitting = $state(false)
    let formRef = $state<HTMLFormElement | null>(null)

    function handleDoclingSwitch(checked: boolean) {
        doclingEnabled = checked
        formRef?.requestSubmit()
    }
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
                        <p class="text-muted-foreground mt-0.5 text-sm">
                            Powered by Docling
                        </p>
                    </div>
                </div>
                <Card.Action>
                    <div class="flex items-center gap-2">
                        <form
                            method="POST"
                            action="?/updateDocling"
                            bind:this={formRef}
                            class="hidden"
                            use:enhance={() => {
                                isSubmitting = true
                                return async ({
                                    result,
                                    update,
                                }: {
                                    result: { type: string; data?: { message?: string; error?: string } }
                                    update: () => Promise<void>
                                }) => {
                                    isSubmitting = false
                                    await update()
                                    if (result.type === 'success') {
                                        toast.success(
                                            result.data?.message || 'Setting updated',
                                        )
                                    } else if (result.type === 'failure') {
                                        toast.error(result.data?.error || 'Something went wrong')
                                        doclingEnabled = data.doclingEnabled
                                    }
                                }
                            }}>
                            <input
                                type="hidden"
                                name="enabled"
                                value={doclingEnabled ? 'true' : 'false'} />
                        </form>
                        <Switch
                            checked={doclingEnabled}
                            disabled={isSubmitting || data.doclingOverriddenByEnv}
                            onCheckedChange={handleDoclingSwitch}
                            class="cursor-pointer" />
                    </div>
                </Card.Action>
            </Card.Header>
            <Card.Content>
                <p class="text-muted-foreground mb-4 text-sm">
                    Uses AI-based layout analysis with built-in OCR to extract text from PDFs,
                    Office documents, and images. Produces structure-aware Markdown that preserves
                    tables, headings, and reading order for higher-quality search results.
                </p>

                {#if data.doclingOverriddenByEnv}
                    <Alert.Root variant="default" class="mb-4">
                        <Info class="h-4 w-4" />
                        <Alert.Title>Controlled by environment variable</Alert.Title>
                        <Alert.Description>
                            This setting is controlled by the
                            <code class="bg-muted rounded px-1 py-0.5 text-sm"
                                >DOCLING_ENABLED={data.doclingEnvValue}</code>
                            environment variable and cannot be changed via the UI.
                        </Alert.Description>
                    </Alert.Root>
                {/if}

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
                            The Docling service is not responding. Make sure it is running:
                            <code class="bg-muted mt-1 block rounded px-2 py-1 text-sm">
                                docker compose --profile docling up -d
                            </code>
                        </Alert.Description>
                    </Alert.Root>
                {/if}
            </Card.Content>
        </Card.Root>
    </div>
</div>
