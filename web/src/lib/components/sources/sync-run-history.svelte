<script lang="ts">
    import * as Card from '$lib/components/ui/card'
    import * as Accordion from '$lib/components/ui/accordion'
    import { Badge } from '$lib/components/ui/badge'
    import {
        formatSyncRunDate,
        formatSyncRunDuration,
        getSyncRunStatusColor,
    } from '$lib/utils/sources'
    import type { SyncRun } from '$lib/server/db/schema'
    import { Info } from '@lucide/svelte'
    import * as Tooltip from '$lib/components/ui/tooltip'

    let { runs = [], timeZone = null }: { runs?: SyncRun[]; timeZone?: string | null } = $props()
</script>

<Card.Root class="Root">
    <Accordion.Root type="single">
        <Accordion.Item value="sync-history" class="border-b-0">
            <Card.Header>
                <Accordion.Trigger class="py-0 hover:no-underline" level={2}>
                    <div class="flex flex-col items-start gap-1">
                        <Card.Title>Sync history</Card.Title>
                        <Card.Description>Latest 10 sync runs for this source</Card.Description>
                    </div>
                </Accordion.Trigger>
            </Card.Header>
            <Accordion.Content class="pb-0">
                <Card.Content class="pt-4">
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
                                        <th class="py-2 pr-4 font-medium">
                                            <div class="flex items-center justify-end gap-1">
                                                <span>Scanned / Processed / Updated</span>
                                                <Tooltip.Provider delayDuration={300}>
                                                    <Tooltip.Root>
                                                        <Tooltip.Trigger
                                                            class="text-muted-foreground hover:text-foreground inline-flex cursor-pointer items-center">
                                                            <Info class="h-3.5 w-3.5" />
                                                            <span class="sr-only"
                                                                >Sync count definitions</span>
                                                        </Tooltip.Trigger>
                                                        <Tooltip.Content
                                                            side="top"
                                                            align="end"
                                                            class="max-w-xs text-left leading-relaxed">
                                                            Scanned counts source items the
                                                            connector inspected. Processed and
                                                            updated count search documents written
                                                            to the index. Some connectors group
                                                            multiple source items into fewer indexed
                                                            documents, so processed can be lower
                                                            than scanned without data being dropped.
                                                        </Tooltip.Content>
                                                    </Tooltip.Root>
                                                </Tooltip.Provider>
                                            </div>
                                        </th>
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
                                            <td class="py-2 pr-4">
                                                <Badge variant="outline" class="capitalize">
                                                    {run.syncType}
                                                </Badge>
                                            </td>
                                            <td class="py-2 pr-4 whitespace-nowrap">
                                                {formatSyncRunDate(run.startedAt, timeZone)}
                                            </td>
                                            <td class="py-2 pr-4 whitespace-nowrap">
                                                {formatSyncRunDuration(
                                                    run.startedAt,
                                                    run.completedAt,
                                                )}
                                            </td>
                                            <td class="py-2 pr-4 text-right whitespace-nowrap">
                                                {#if run.status.toLowerCase() === 'failed'}
                                                    <span class="text-muted-foreground">-</span>
                                                {:else}
                                                    {(run.documentsScanned ?? 0).toLocaleString()} / {(
                                                        run.documentsProcessed ?? 0
                                                    ).toLocaleString()} / {(
                                                        run.documentsUpdated ?? 0
                                                    ).toLocaleString()}
                                                {/if}
                                            </td>
                                            <td class="max-w-xs py-2">
                                                {#if run.errorMessage}
                                                    <span
                                                        class="line-clamp-2 break-words text-red-600"
                                                        title={run.errorMessage}>
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
                </Card.Content>
            </Accordion.Content>
        </Accordion.Item>
    </Accordion.Root>
</Card.Root>
