<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { Loader2 } from '@lucide/svelte'

    type UploadMeta = {
        filename: string
        contentType: string
        sizeBytes: number
    }

    interface Props {
        uploadId?: string
        filename?: string
        uploading?: boolean
        onRemove?: () => void
    }

    let { uploadId, filename, uploading = false, onRemove }: Props = $props()

    async function fetchMeta(id: string): Promise<UploadMeta> {
        const resp = await fetch(`/api/uploads/${id}`)
        if (!resp.ok) throw new Error(`status ${resp.status}`)
        return resp.json()
    }

    let metaPromise = $derived(uploadId && !filename ? fetchMeta(uploadId) : null)

    function getExtension(name: string): string {
        const dot = name.lastIndexOf('.')
        return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1) : ''
    }
</script>

{#snippet card(name: string, isUploading: boolean)}
    <div
        class="bg-muted/80 border-primary/10 flex flex-row items-center justify-between rounded-lg border px-4 py-3 text-sm shadow-sm">
        <div class="flex min-w-0 items-center gap-2">
            {#if isUploading}
                <Loader2 class="text-muted-foreground size-4 shrink-0 animate-spin" />
            {/if}
            <div class="truncate pr-4 font-medium break-all">{name}</div>
        </div>
        {#if onRemove}
            <button
                aria-label="Remove"
                class="text-muted-foreground hover:text-foreground cursor-pointer"
                onclick={onRemove}>×</button>
        {/if}
    </div>
{/snippet}

{#if filename}
    {@render card(filename, uploading)}
{:else if metaPromise}
    {#await metaPromise}
        {@render card('loading…', false)}
    {:then meta}
        {@render card(meta.filename, false)}
    {:catch}
        {@render card('attachment unavailable', false)}
    {/await}
{/if}
