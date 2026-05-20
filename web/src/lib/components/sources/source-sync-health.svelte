<script lang="ts">
    import * as Alert from '$lib/components/ui/alert'
    import { AlertTriangle } from '@lucide/svelte'
    import type { SourceSyncOverview } from '$lib/types'

    let { overview }: { overview: SourceSyncOverview | null } = $props()

    const latestFailedRun = $derived(
        overview?.sync_runs.find((run) => run.status.toLowerCase() === 'failed'),
    )
</script>

{#if overview?.health === 'unhealthy'}
    <Alert.Root variant="destructive">
        <AlertTriangle class="h-4 w-4" />
        <Alert.Title>Source unhealthy</Alert.Title>
        <Alert.Description>
            Scheduled syncs have been paused after repeated failures.
            {#if latestFailedRun?.error_message}
                <span class="block break-words">{latestFailedRun.error_message}</span>
            {/if}
        </Alert.Description>
    </Alert.Root>
{/if}
