<script lang="ts">
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
        class="bg-muted relative flex h-14 w-44 shrink-0 flex-col justify-between rounded-md border px-2.5 py-1.5 text-xs shadow-sm">
        {#if onRemove}
            <button
                type="button"
                aria-label="Remove"
                class="text-muted-foreground hover:text-foreground absolute top-1 right-1 cursor-pointer leading-none"
                onclick={onRemove}>×</button>
        {/if}
        <span class="line-clamp-2 pr-4 font-medium break-all">{name}</span>
        <div
            class="text-muted-foreground flex items-center justify-end gap-1 text-[10px] uppercase">
            {#if isUploading}
                <span>uploading…</span>
            {:else}
                {@const ext = getExtension(name)}
                {#if ext}<span>{ext}</span>{/if}
            {/if}
        </div>
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
