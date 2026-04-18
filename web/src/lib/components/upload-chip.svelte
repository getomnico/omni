<script lang="ts">
    import { Loader2 } from '@lucide/svelte'

    interface Props {
        filename?: string
        uploading?: boolean
        onRemove?: () => void
    }

    let { filename, uploading = false, onRemove }: Props = $props()
    let loading = $derived(uploading || filename === undefined)
</script>

<div
    class="bg-muted/80 border-primary/20 flex flex-row items-center justify-between rounded-lg border px-4 py-3 text-sm shadow-sm">
    <div class="flex min-w-0 items-center gap-2">
        {#if loading}
            <Loader2 class="text-muted-foreground size-4 shrink-0 animate-spin" />
        {/if}
        {#if filename}
            <div class="max-w-48 truncate pr-4 font-medium break-all">{filename}</div>
        {/if}
    </div>
    {#if onRemove}
        <button
            aria-label="Remove"
            class="text-muted-foreground hover:text-foreground cursor-pointer"
            onclick={onRemove}>×</button>
    {/if}
</div>
