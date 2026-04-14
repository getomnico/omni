<script lang="ts">
    import { Paperclip } from '@lucide/svelte'

    type Meta = {
        filename: string
        contentType: string
        sizeBytes: number
    }

    let { uploadId }: { uploadId: string } = $props()
    let meta = $state<Meta | null>(null)
    let error = $state(false)

    $effect(() => {
        let cancelled = false
        ;(async () => {
            try {
                const resp = await fetch(`/api/uploads/${uploadId}`)
                if (!resp.ok) {
                    if (!cancelled) error = true
                    return
                }
                const data = await resp.json()
                if (!cancelled) meta = data
            } catch {
                if (!cancelled) error = true
            }
        })()
        return () => {
            cancelled = true
        }
    })

    function formatSize(n: number): string {
        if (n < 1024) return `${n} B`
        if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`
        return `${(n / (1024 * 1024)).toFixed(1)} MB`
    }
</script>

<div class="bg-background flex max-w-xs items-center gap-2 rounded-lg border px-3 py-2 text-sm">
    <Paperclip class="h-4 w-4 shrink-0 text-gray-500" />
    {#if meta}
        <div class="flex min-w-0 flex-col">
            <span class="truncate font-medium">{meta.filename}</span>
            <span class="text-xs text-gray-500">{formatSize(meta.sizeBytes)}</span>
        </div>
    {:else if error}
        <span class="text-gray-500">attachment unavailable</span>
    {:else}
        <span class="text-gray-500">loading…</span>
    {/if}
</div>
