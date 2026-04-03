<script lang="ts">
    import { enhance } from '$app/forms'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Switch } from '$lib/components/ui/switch'
    import { Info, FileText, Sparkles, AlertTriangle } from '@lucide/svelte'
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

        <div class="space-y-4">
            <!-- Docling AI-based Conversion -->
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
                                Use Docling for superior document extraction
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
                    {#if data.doclingOverriddenByEnv}
                        <Alert.Root variant="default" class="mb-4">
                            <Info class="h-4 w-4" />
                            <Alert.Title>Controlled by environment variable</Alert.Title>
                            <Alert.Description>
                                This setting is currently controlled by the
                                <code class="bg-muted rounded px-1 py-0.5 text-sm"
                                    >DOCLING_ENABLED={data.doclingEnvValue}</code>
                                environment variable and cannot be changed via the UI.
                            </Alert.Description>
                        </Alert.Root>
                    {/if}

                    <div class="space-y-4">
                        <div class="text-muted-foreground text-sm">
                            <p class="mb-3">
                                When enabled, all document conversions will use Docling's AI-based
                                extraction pipeline instead of the built-in lightweight extractors.
                            </p>

                            <div class="space-y-4">
                                <div>
                                    <h4 class="text-foreground mb-2 font-medium">Advantages</h4>
                                    <ul class="list-inside list-disc space-y-1">
                                        <li>
                                            <strong>Superior PDF extraction</strong> — AI-based layout
                                            analysis correctly handles tables, multi-column layouts, and
                                            reading order
                                        </li>
                                        <li>
                                            <strong>Built-in OCR</strong> — Scanned PDFs and image files
                                            become fully searchable
                                        </li>
                                        <li>
                                            <strong>Structure-aware output</strong> — Preserves headings,
                                            sections, and table structure for better RAG chunking
                                        </li>
                                        <li>
                                            <strong>Broad format support</strong> — PDF, DOCX, XLSX, PPTX,
                                            HTML, images (PNG, JPEG, TIFF, BMP, WEBP), and more
                                        </li>
                                    </ul>
                                </div>

                                <div>
                                    <h4 class="text-foreground mb-2 font-medium">Trade-offs</h4>
                                    <ul class="list-inside list-disc space-y-1">
                                        <li>
                                            <strong>Slower processing</strong> — AI-based extraction takes
                                            seconds per document vs milliseconds for simple text extraction
                                        </li>
                                        <li>
                                            <strong>Resource intensive</strong> — Requires the Docling service
                                            to be running (enabled via the
                                            <code class="bg-muted rounded px-1 py-0.5">docling</code> Docker
                                            Compose profile)
                                        </li>
                                        <li>
                                            <strong>GPU recommended</strong> — CPU-only mode works but is
                                            slow; GPU acceleration is recommended for production
                                        </li>
                                    </ul>
                                </div>
                            </div>
                        </div>

                        {#if doclingEnabled}
                            <Alert.Root variant="default">
                                <AlertTriangle class="h-4 w-4" />
                                <Alert.Title>Docling service required</Alert.Title>
                                <Alert.Description>
                                    Make sure the Docling service is running. Start it with:
                                    <code class="bg-muted mt-1 block rounded px-2 py-1 text-sm">
                                        docker compose --profile docling up -d
                                    </code>
                                </Alert.Description>
                            </Alert.Root>
                        {/if}
                    </div>
                </Card.Content>
            </Card.Root>

            <!-- Built-in Extraction Info -->
            <Card.Root>
                <Card.Header>
                    <div class="flex items-center gap-3">
                        <div
                            class="bg-muted flex h-10 w-10 items-center justify-center rounded-lg">
                            <FileText class="text-muted-foreground h-5 w-5" />
                        </div>
                        <div>
                            <div class="text-base leading-tight font-semibold">
                                Built-in Document Extraction
                            </div>
                            <p class="text-muted-foreground mt-0.5 text-sm">
                                Default lightweight extraction (currently {doclingEnabled
                                    ? 'disabled'
                                    : 'active'})
                            </p>
                        </div>
                    </div>
                </Card.Header>
                <Card.Content>
                    <div class="text-muted-foreground text-sm">
                        <p class="mb-3">
                            The built-in extractors are lightweight and fast, suitable for simple
                            text-heavy documents.
                        </p>

                        <div class="overflow-x-auto">
                            <table class="w-full text-left text-sm">
                                <thead>
                                    <tr class="border-b">
                                        <th class="pb-2 font-medium">Format</th>
                                        <th class="pb-2 font-medium">Library</th>
                                        <th class="pb-2 font-medium">Limitations</th>
                                    </tr>
                                </thead>
                                <tbody class="divide-y">
                                    <tr>
                                        <td class="py-2">PDF</td>
                                        <td class="py-2">
                                            <code class="bg-muted rounded px-1">pdf-oxide</code>
                                        </td>
                                        <td class="py-2">No OCR, tables garbled, layout issues</td>
                                    </tr>
                                    <tr>
                                        <td class="py-2">DOCX</td>
                                        <td class="py-2">
                                            <code class="bg-muted rounded px-1">docx-rs</code>
                                        </td>
                                        <td class="py-2">Basic text only</td>
                                    </tr>
                                    <tr>
                                        <td class="py-2">XLSX</td>
                                        <td class="py-2">
                                            <code class="bg-muted rounded px-1">calamine</code>
                                        </td>
                                        <td class="py-2">Cell values only, no formatting</td>
                                    </tr>
                                    <tr>
                                        <td class="py-2">PPTX</td>
                                        <td class="py-2">
                                            <code class="bg-muted rounded px-1">quick-xml</code>
                                        </td>
                                        <td class="py-2">Text only, no images</td>
                                    </tr>
                                    <tr>
                                        <td class="py-2">HTML</td>
                                        <td class="py-2">
                                            <code class="bg-muted rounded px-1">html2text</code>
                                        </td>
                                        <td class="py-2">Plain text conversion</td>
                                    </tr>
                                    <tr>
                                        <td class="py-2">Images</td>
                                        <td class="py-2">—</td>
                                        <td class="py-2 text-amber-600">Not supported</td>
                                    </tr>
                                </tbody>
                            </table>
                        </div>
                    </div>
                </Card.Content>
            </Card.Root>
        </div>
    </div>
</div>
