<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import ImapCitationSource from '$lib/components/search-results/imap-citation-source.svelte'
    import { documentPreviewSchema, type DocumentPreview } from '$lib/types/document-preview'
    import type { UserConfiguration } from '$lib/types/configuration'
    import type { NormalizedCitation } from '$lib/utils/citations'
    import { formatDateTime } from '$lib/utils/datetime'
    import { getSourceDisplayName } from '$lib/utils/icons'
    import type { SourceType } from '$lib/types'
    import { FileText, Loader2, Mail } from '@lucide/svelte'

    type Props = {
        open: boolean
        citation: NormalizedCitation | null
        /** Indexed document the citation resolved to; null when it could not be resolved. */
        documentId: string | null
        userConfiguration?: UserConfiguration | null
    }

    type LoadState =
        | { kind: 'idle' }
        | { kind: 'loading' }
        | { kind: 'loaded'; preview: DocumentPreview }
        | { kind: 'failed'; message: string }

    let { open = $bindable(), citation, documentId, userConfiguration = null }: Props = $props()

    let loadState = $state<LoadState>({ kind: 'idle' })

    async function loadPreview(id: string, signal: AbortSignal): Promise<LoadState> {
        const response = await fetch(`/api/documents/${encodeURIComponent(id)}`, { signal })
        if (response.status === 404) {
            return { kind: 'failed', message: 'The stored document is no longer available.' }
        }
        if (!response.ok) {
            return { kind: 'failed', message: 'The stored document could not be loaded.' }
        }
        const parsed = documentPreviewSchema.safeParse(await response.json())
        if (!parsed.success) {
            return { kind: 'failed', message: 'The stored document could not be loaded.' }
        }
        return { kind: 'loaded', preview: parsed.data }
    }

    $effect(() => {
        // Keep the last state while closed so the closing animation doesn't flicker.
        if (!open) return
        if (!documentId) {
            loadState = { kind: 'idle' }
            return
        }

        const controller = new AbortController()
        loadState = { kind: 'loading' }
        loadPreview(documentId, controller.signal)
            .then((state) => {
                if (!controller.signal.aborted) loadState = state
            })
            .catch((error: unknown) => {
                if (controller.signal.aborted) return
                console.error('Failed to load cited document', error)
                loadState = { kind: 'failed', message: 'The stored document could not be loaded.' }
            })

        return () => controller.abort()
    })

    let preview = $derived(loadState.kind === 'loaded' ? loadState.preview : null)
    let email = $derived(preview?.email ?? null)
    let isEmail = $derived(email !== null || citation?.isImap === true)
    // Citations of read_document results carry no source type, so prefer the stored one.
    let sourceName = $derived(
        (preview?.sourceType && getSourceDisplayName(preview.sourceType as SourceType)) ||
            citation?.sourceName,
    )
</script>

{#snippet headerField(label: string, value: string)}
    <dt class="text-muted-foreground font-medium">{label}</dt>
    <dd class="min-w-0 break-words">{value}</dd>
{/snippet}

<Dialog.Root bind:open>
    <Dialog.Content class="flex flex-col gap-4 sm:max-w-2xl">
        {#if citation}
            <Dialog.Header class="min-w-0 pr-6 text-left">
                <div class="text-muted-foreground flex items-center gap-1.5 text-xs font-semibold">
                    {#if isEmail}
                        <Mail class="h-4 w-4 flex-shrink-0" />
                    {:else}
                        <FileText class="h-4 w-4 flex-shrink-0" />
                    {/if}
                    {sourceName}
                </div>
                <Dialog.Title class="text-base leading-snug break-words">
                    {preview?.title ?? citation.title}
                </Dialog.Title>
                <Dialog.Description class="sr-only"
                    >Stored content of the cited source</Dialog.Description>
                {#if !email && citation.isImap}
                    <ImapCitationSource source={citation.iconHint ?? undefined} />
                {/if}
            </Dialog.Header>

            {#if email}
                <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
                    {@render headerField('From', email.from)}
                    {#if email.to.length > 0}
                        {@render headerField('To', email.to.join(', '))}
                    {/if}
                    {#if email.cc.length > 0}
                        {@render headerField('Cc', email.cc.join(', '))}
                    {/if}
                    {#if preview?.date}
                        {@render headerField(
                            'Date',
                            formatDateTime(preview.date, userConfiguration),
                        )}
                    {/if}
                    {#if email.account || email.folder}
                        {@render headerField(
                            'Mailbox',
                            [email.account, email.folder].filter(Boolean).join(' · '),
                        )}
                    {/if}
                    {#if email.messageCount !== null && email.messageCount > 1}
                        {@render headerField('Messages', String(email.messageCount))}
                    {/if}
                </dl>
            {:else if preview?.date}
                <p class="text-muted-foreground text-xs">
                    {formatDateTime(preview.date, userConfiguration)}
                </p>
            {/if}

            {#if loadState.kind === 'loading'}
                <div class="text-muted-foreground flex items-center gap-2 text-sm">
                    <Loader2 class="h-4 w-4 animate-spin" />
                    Loading stored content…
                </div>
            {:else if preview}
                <div
                    data-testid="citation-document-content"
                    class="bg-muted/30 max-h-[55dvh] min-h-0 overflow-y-auto rounded-md border p-3 text-sm break-words whitespace-pre-wrap">
                    {preview.content}
                </div>
                {#if preview.isTruncated}
                    <p class="text-muted-foreground text-xs">
                        Only the beginning of this document is shown.
                    </p>
                {/if}
            {:else}
                {#if loadState.kind === 'failed'}
                    <p class="text-muted-foreground text-xs">{loadState.message}</p>
                {/if}
                <div class="flex flex-col gap-1">
                    <p class="text-muted-foreground text-xs font-medium">Cited excerpt</p>
                    <div
                        class="bg-muted/30 max-h-[55dvh] overflow-y-auto rounded-md border p-3 text-sm break-words whitespace-pre-wrap">
                        {citation.citedText}
                    </div>
                </div>
            {/if}
        {/if}
    </Dialog.Content>
</Dialog.Root>
