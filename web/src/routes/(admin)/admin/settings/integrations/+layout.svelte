<script lang="ts">
    import SourceSyncHealth from '$lib/components/sources/source-sync-health.svelte'
    import SyncRunHistory from '$lib/components/sources/sync-run-history.svelte'
    import { ArrowLeft } from '@lucide/svelte'
    import type { Snippet } from 'svelte'
    import type { LayoutData } from './$types.js'

    interface Props {
        data: LayoutData
        children: Snippet
    }

    let { data, children }: Props = $props()
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
            <SyncRunHistory runs={data.syncRuns} />

            {@render children()}
        </div>
    </div>
{:else}
    {@render children()}
{/if}
