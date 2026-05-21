<script lang="ts">
    import * as Alert from '$lib/components/ui/alert'
    import { AlertCircle } from '@lucide/svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { health, syncRuns = [] }: { health: 'healthy' | 'unhealthy'; syncRuns?: SyncRun[] } =
        $props()

    const latestFailedRun = $derived(syncRuns.find((run) => run.status.toLowerCase() === 'failed'))
</script>

{#if health === 'unhealthy'}
    <Alert.Root
        class="border-red-200 bg-red-50 text-red-900 dark:border-red-900 dark:bg-red-950 dark:text-red-50">
        <AlertCircle class="h-4 w-4" />
        <Alert.Title>Source unhealthy</Alert.Title>
        <Alert.Description>
            Scheduled syncs have been paused after repeated failures.
            {#if latestFailedRun?.errorMessage}
                <div class="mt-3 space-y-1">
                    <div class="text-xs font-medium">Error message</div>
                    <code
                        class="block max-h-48 overflow-y-auto font-mono text-xs break-words whitespace-pre-wrap">
                        {latestFailedRun.errorMessage}
                    </code>
                </div>
            {/if}
        </Alert.Description>
    </Alert.Root>
{/if}
