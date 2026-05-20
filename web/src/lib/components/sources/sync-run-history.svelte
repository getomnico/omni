<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import {
        formatSyncRunDate,
        formatSyncRunDuration,
        getSyncRunStatusColor,
    } from '$lib/utils/sources'
    import type { SourceSyncRun } from '$lib/types'

    let { runs = [] }: { runs?: SourceSyncRun[] } = $props()
</script>

<Accordion.Root type="single" class="bg-card text-card-foreground rounded-md border shadow-sm">
    <Accordion.Item value="sync-history" class="border-b-0">
        <Accordion.Trigger class="px-4 py-3 hover:no-underline">
            <div class="flex flex-col items-start gap-1">
                <span class="font-semibold">Sync history</span>
                <span class="text-muted-foreground text-sm font-normal">
                    Latest 10 sync runs for this source
                </span>
            </div>
        </Accordion.Trigger>
        <Accordion.Content class="px-4 pb-4">
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
                                    <td class="py-2 pr-4">{run.syncType}</td>
                                    <td class="py-2 pr-4 whitespace-nowrap">
                                        {formatSyncRunDate(run.startedAt)}
                                    </td>
                                    <td class="py-2 pr-4 whitespace-nowrap">
                                        {formatSyncRunDuration(run.startedAt, run.completedAt)}
                                    </td>
                                    <td class="py-2 pr-4 text-right">
                                        {(run.documentsScanned ?? 0).toLocaleString()}
                                    </td>
                                    <td class="py-2 pr-4 text-right">
                                        {(run.documentsProcessed ?? 0).toLocaleString()}
                                    </td>
                                    <td class="py-2 pr-4 text-right">
                                        {(run.documentsUpdated ?? 0).toLocaleString()}
                                    </td>
                                    <td class="max-w-xs py-2">
                                        {#if run.errorMessage}
                                            <span class="line-clamp-2 break-words text-red-600">
                                                {run.errorMessage}
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
        </Accordion.Content>
    </Accordion.Item>
</Accordion.Root>
