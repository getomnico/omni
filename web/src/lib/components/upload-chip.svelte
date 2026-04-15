<script lang="ts">
    import { Paperclip } from '@lucide/svelte'

    type UploadMeta = {
        filename: string
        contentType: string
        sizeBytes: number
    }

    let { uploadId }: { uploadId: string } = $props()

    async function fetchMeta(id: string): Promise<UploadMeta> {
        const resp = await fetch(`/api/uploads/${id}`)
        if (!resp.ok) throw new Error(`status ${resp.status}`)
        return resp.json()
    }

    let metaPromise = $derived(fetchMeta(uploadId))

    function formatSize(n: number): string {
        if (n < 1024) return `${n} B`
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`
        return `${(n / (1024 * 1024)).toFixed(1)} MB`
    }
</script>

<div class="bg-background flex max-w-xs items-center gap-2 rounded-lg border px-3 py-2 text-sm">
    <Paperclip class="h-4 w-4 shrink-0 text-gray-500" />
    {#await metaPromise}
        <span class="text-gray-500">loading…</span>
    {:then meta}
        <div class="flex min-w-0 flex-col">
            <span class="truncate font-medium">{meta.filename}</span>
            <span class="text-xs text-gray-500">{formatSize(meta.sizeBytes)}</span>
        </div>
    {:catch}
        <span class="text-gray-500">attachment unavailable</span>
    {/await}
</div>
