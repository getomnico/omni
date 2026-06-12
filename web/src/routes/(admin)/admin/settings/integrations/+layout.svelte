<script lang="ts">
    import SourceSyncHealth from '$lib/components/sources/source-sync-health.svelte'
    import SourceSyncIntervalCard from '$lib/components/sources/source-sync-interval-card.svelte'
    import SyncRunHistory from '$lib/components/sources/sync-run-history.svelte'
    import { Button } from '$lib/components/ui/button'
    import * as Card from '$lib/components/ui/card'
    import { ArrowLeft } from '@lucide/svelte'
    import RemoveSourceDialog from './remove-source-dialog.svelte'
    import type { Snippet } from 'svelte'
    import type { LayoutData } from './$types.js'

    interface Props {
        data: LayoutData
        children: Snippet
    }

    let { data, children }: Props = $props()
    let showRemoveDialog = $state(false)
</script>

{#if data.source}
    <div class="h-full overflow-y-auto p-6 py-8 pb-24">
        <div class="mx-auto max-w-screen-lg space-y-4">
            <a
                href="/admin/settings/integrations"
                class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors">
                <ArrowLeft class="h-4 w-4" />
                Back to Integrations
            </a>

            <SourceSyncHealth health={data.health} syncRuns={data.syncRuns} />

            {@render children()}

            <SourceSyncIntervalCard
                sourceId={data.source.id}
                syncIntervalSeconds={data.source.syncIntervalSeconds} />

            <SyncRunHistory runs={data.syncRuns} />

            <Card.Root>
                <Card.Content class="flex items-center justify-between">
                    <div>
                        <Card.Title>Delete Source</Card.Title>
                        <Card.Description>
                            Permanently delete this source and all its synced data, credentials, and
                            sync history
                        </Card.Description>
                    </div>
                    <Button
                        variant="destructive"
                        class="cursor-pointer"
                        onclick={() => (showRemoveDialog = true)}>
                        Delete Permanently
                    </Button>
                </Card.Content>
            </Card.Root>
        </div>
    </div>

    <RemoveSourceDialog
        bind:open={showRemoveDialog}
        sourceId={data.source.id}
        sourceName={data.source.name} />
{:else}
    {@render children()}
{/if}
