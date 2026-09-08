<script lang="ts">
    import { CircleAlert, Download, LoaderCircle } from '@lucide/svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'

    type Props = {
        state: 'loading' | 'error'
        message?: string
        artifact?: ArtifactData
    }

    let { state, message, artifact }: Props = $props()
</script>

<div class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
    {#if state === 'loading'}
        <LoaderCircle class="text-muted-foreground h-6 w-6 animate-spin" />
        <p class="text-muted-foreground text-sm">{message ?? 'Loading preview…'}</p>
    {:else}
        <CircleAlert class="text-destructive h-6 w-6" />
        <p class="text-sm font-medium">Preview unavailable</p>
        {#if message}
            <p class="text-muted-foreground max-w-sm text-xs">{message}</p>
        {/if}
        {#if artifact}
            <a
                href={artifact.url}
                download
                rel="external"
                class="text-primary hover:text-primary/80 inline-flex cursor-pointer items-center gap-1.5 text-sm font-medium no-underline">
                <Download class="h-4 w-4" />
                Download file
            </a>
        {/if}
    {/if}
</div>
