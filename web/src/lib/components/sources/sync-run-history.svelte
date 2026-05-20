<script lang="ts">
    import * as Card from '$lib/components/ui/card'
    import {
        formatSyncRunDate,
        formatSyncRunDuration,
        getSyncRunStatusColor,
    } from '$lib/utils/sources'
    import type { ConnectorManagerSyncRun } from '$lib/types'

    let { runs = [] }: { runs?: ConnectorManagerSyncRun[] } = $props()
</script>

<Card.Root>
    <Card.Header>
        <Card.Title>Sync history</Card.Title>
        <Card.Description>Latest 10 sync runs for this source</Card.Description>
    </Card.Header>
    <Card.Content>
        {#if runs.length === 0}
            <p class="text-muted-foreground text-sm">No sync runs yet.</p>
        {:else}
            <div class="overflow-x-auto">
                <table class="w-full text-sm">
                    <thead class="text-muted-foreground border-b text-left text-xs">
                        <tr>
                            <th class="py-2 pr-4 font-medium">Status</th>
                            <th class="py-2 pr-4 font-medium">Type</th>
                            <th class="py-2 pr-4 font-medium">Started</th>
                            <th class="py-2 pr-4 font-medium">Duration</th>
                            <th class="py-2 pr-4 text-right font-medium">Scanned</th>
                            <th class="py-2 pr-4 text-right font-medium">Processed</th>
                            <th class="py-2 pr-4 text-right font-medium">Updated</th>
                            <th class="py-2 font-medium">Error</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y">
                        {#each runs as run}
                            <tr>
                                <td class="py-2 pr-4">
                                    <span
                                        class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${getSyncRunStatusColor(run.status)}`}>
                                        {run.status}
                                    </span>
                                </td>
                                <td class="py-2 pr-4">{run.sync_type}</td>
                                <td class="py-2 pr-4 whitespace-nowrap">
                                    {formatSyncRunDate(run.started_at)}
                                </td>
                                <td class="py-2 pr-4 whitespace-nowrap">
                                    {formatSyncRunDuration(run.started_at, run.completed_at)}
                                </td>
                                <td class="py-2 pr-4 text-right">
                                    {run.documents_scanned.toLocaleString()}
                                </td>
                                <td class="py-2 pr-4 text-right">
                                    {run.documents_processed.toLocaleString()}
                                </td>
                                <td class="py-2 pr-4 text-right">
                                    {run.documents_updated.toLocaleString()}
                                </td>
                                <td class="max-w-xs py-2">
                                    {#if run.error_message}
                                        <span class="line-clamp-2 break-words text-red-600">
                                            {run.error_message}
                                        </span>
                                    {:else}
                                        <span class="text-muted-foreground">-</span>
                                    {/if}
                                </td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        {/if}
    </Card.Content>
</Card.Root>
