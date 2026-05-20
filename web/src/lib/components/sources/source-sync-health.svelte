<script lang="ts">
    import * as Alert from '$lib/components/ui/alert'
    import { AlertTriangle } from '@lucide/svelte'
    import type { SourceSyncRun } from '$lib/types'

    let { health, syncRuns = [] }: { health: 'healthy' | 'unhealthy'; syncRuns?: SourceSyncRun[] } =
        $props()

    const latestFailedRun = $derived(syncRuns.find((run) => run.status.toLowerCase() === 'failed'))
</script>

{#if health === 'unhealthy'}
    <Alert.Root variant="destructive">
        <AlertTriangle class="h-4 w-4" />
        <Alert.Title>Source unhealthy</Alert.Title>
        <Alert.Description>
            Scheduled syncs have been paused after repeated failures.
            {#if latestFailedRun?.errorMessage}
                <span class="block break-words">{latestFailedRun.errorMessage}</span>
            {/if}
        </Alert.Description>
    </Alert.Root>
{/if}
