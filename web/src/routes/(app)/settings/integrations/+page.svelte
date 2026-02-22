<script lang="ts">
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
        CardFooter,
    } from '$lib/components/ui/card'
    import { Button } from '$lib/components/ui/button'
    import type { PageProps } from './$types'
    import googleLogo from '$lib/images/icons/google.svg'
    import { Globe, HardDrive } from '@lucide/svelte'
    import GoogleOAuthSetup from '$lib/components/google-oauth-setup.svelte'
    import { getSourceIconPath } from '$lib/utils/icons'
    import { enhance } from '$app/forms'
    import { SourceType } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import { invalidateAll } from '$app/navigation'
    import { onMount, onDestroy } from 'svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { data }: PageProps = $props()

    let showGoogleOAuthSetup = $state(false)

    let hasGoogleSource = $derived(
        data.connectedSources.some(
            (s) => s.sourceType === 'google_drive' || s.sourceType === 'gmail',
        ),
    )

    type SourceId = string
    let latestSyncRuns = $state<Map<SourceId, SyncRun>>(data.latestSyncRuns)
    let documentCounts = $state<Record<SourceId, number>>({})
    let eventSource = $state<EventSource | null>(null)

    $effect(() => {
        latestSyncRuns = data.latestSyncRuns
    })

    onMount(() => {
        eventSource = new EventSource('/api/indexing/status')
        eventSource.onmessage = (event) => {
            try {
                const statusData = JSON.parse(event.data)
                if (statusData.overall?.latestSyncRuns) {
                    const updated = new Map(latestSyncRuns)
                    statusData.overall.latestSyncRuns.forEach((sync: any) => {
                        if (sync.sourceId) {
                            updated.set(sync.sourceId, sync)
                        }
                    })
                    latestSyncRuns = updated
                }
                if (statusData.overall?.documentCounts) {
                    documentCounts = statusData.overall.documentCounts
                }
            } catch (error) {
                console.error('Error parsing SSE data:', error)
            }
        }

        eventSource.onerror = (error) => {
            console.error('EventSource error:', error)
        }
    })

    onDestroy(() => {
        if (eventSource) {
            eventSource.close()
        }
    })

    async function handleSync(sourceId: string) {
        try {
            const response = await fetch(`/api/sources/${sourceId}/sync`, {
                method: 'POST',
            })
            if (!response.ok) {
                toast.error('Failed to trigger sync')
            } else {
                toast.success('Sync triggered successfully')
                await invalidateAll()
            }
        } catch (error) {
            console.error('Error triggering sync:', error)
            toast.error('Failed to trigger sync')
        }
    }

    function handleGoogleOAuthSetupSuccess() {
        showGoogleOAuthSetup = false
        window.location.reload()
    }

    function getSourceNoun(sourceType: SourceType): string {
        switch (sourceType) {
            case SourceType.GOOGLE_DRIVE:
                return 'documents'
            case SourceType.GMAIL:
                return 'threads'
            case SourceType.SLACK:
                return 'messages'
            case SourceType.CONFLUENCE:
                return 'pages'
            case SourceType.JIRA:
                return 'issues'
            case SourceType.HUBSPOT:
                return 'records'
            case SourceType.FIREFLIES:
                return 'transcripts'
            case SourceType.WEB:
                return 'pages'
            case SourceType.LOCAL_FILES:
                return 'files'
            default:
                return 'documents'
        }
    }

    function formatDate(date: Date | null) {
        if (!date) return 'Never'
        return new Date(date).toLocaleString()
    }

    function getStatusColor(isActive: boolean) {
        return isActive
            ? 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
            : 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-300'
    }
</script>

<svelte:head>
    <title>Integrations - Settings</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <!-- Page Header -->
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Integrations</h1>
            <p class="text-muted-foreground mt-2">Apps that are currently connected with Omni</p>
        </div>

        <!-- Connected Sources Section -->
        {#if data.connectedSources.length > 0}
            <div class="space-y-4">
                <div class="space-y-2">
                    {#each data.connectedSources as source}
                        {@const noun = getSourceNoun(source.sourceType as SourceType)}
                        {@const sync = latestSyncRuns.get(source.id)}
                        <div
                            class="flex items-center justify-between gap-4 rounded-lg border px-4 py-3">
                            <div class="flex flex-1 items-start gap-3">
                                {#if getSourceIconPath(source.sourceType)}
                                    <img
                                        src={getSourceIconPath(source.sourceType)}
                                        alt={source.name}
                                        class="h-6 w-6" />
                                {:else if source.sourceType === 'web'}
                                    <Globe class="text-muted-foreground h-6 w-6" />
                                {:else if source.sourceType === 'local_files'}
                                    <HardDrive class="text-muted-foreground h-6 w-6" />
                                {/if}
                                <div class="flex flex-col gap-0.5">
                                    <div class="flex items-center gap-2">
                                        <span class="truncate overflow-hidden font-medium"
                                            >{source.name}</span>
                                        <span
                                            class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(source.isActive)}`}>
                                            {source.isActive ? 'Enabled' : 'Disabled'}
                                        </span>
                                    </div>
                                    <div
                                        class="text-muted-foreground flex items-center gap-1 text-xs">
                                        {#if sync?.status === 'running'}
                                            {#if sync.documentsScanned && sync.documentsScanned > 0}
                                                <span
                                                    >Syncing... {sync.documentsScanned.toLocaleString()}
                                                    {noun} scanned{#if sync.documentsUpdated && sync.documentsUpdated > 0},
                                                        {sync.documentsUpdated.toLocaleString()} updated{/if}</span>
                                            {:else}
                                                <span>Syncing...</span>
                                            {/if}
                                        {:else}
                                            <span
                                                >Last sync: {formatDate(
                                                    sync?.completedAt ?? null,
                                                )}</span>
                                        {/if}
                                        {#if documentCounts[source.id]}
                                            <span class="text-muted-foreground">&middot;</span>
                                            <span
                                                >{documentCounts[source.id].toLocaleString()}
                                                {noun} indexed</span>
                                        {/if}
                                    </div>
                                </div>
                            </div>
                            <div class="flex gap-2">
                                {#if source.isActive}
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        class="cursor-pointer"
                                        disabled={latestSyncRuns.get(source.id)?.status ===
                                            'running'}
                                        onclick={() => handleSync(source.id)}>
                                        Sync
                                    </Button>
                                {/if}
                                <form
                                    method="POST"
                                    action="?/{source.isActive ? 'disable' : 'enable'}"
                                    use:enhance>
                                    <input type="hidden" name="sourceId" value={source.id} />
                                    <Button
                                        type="submit"
                                        variant={source.isActive ? 'outline' : 'default'}
                                        size="sm"
                                        class="cursor-pointer">
                                        {source.isActive ? 'Disable' : 'Enable'}
                                    </Button>
                                </form>
                            </div>
                        </div>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- Available Connections -->
        {#if data.googleOAuthConfigured}
            <div class="space-y-4">
                <div>
                    <h2 class="text-xl font-semibold">Available Connections</h2>
                    <p class="text-muted-foreground text-sm">
                        Connect your own accounts to sync data with Omni
                    </p>
                </div>

                <div class="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
                    <Card class="flex flex-col">
                        <CardHeader>
                            <CardTitle class="flex items-center gap-3">
                                <img src={googleLogo} alt="Google" class="h-6 w-6" />
                                <span>Google</span>
                            </CardTitle>
                            <CardDescription>
                                Connect your Google Drive and Gmail with read-only access. Your data
                                stays private to you.
                            </CardDescription>
                        </CardHeader>
                        <CardContent class="flex-1" />
                        <CardFooter>
                            {#if hasGoogleSource}
                                <span class="text-sm font-medium text-green-500"> Connected </span>
                            {:else}
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => (showGoogleOAuthSetup = true)}>
                                    Connect with Google
                                </Button>
                            {/if}
                        </CardFooter>
                    </Card>
                </div>
            </div>
        {:else if data.connectedSources.length === 0}
            <div class="py-12 text-center">
                <p class="text-muted-foreground text-sm">
                    No integrations are available yet. Contact your administrator to set up
                    connections.
                </p>
            </div>
        {/if}
    </div>
</div>

<GoogleOAuthSetup
    bind:open={showGoogleOAuthSetup}
    onSuccess={handleGoogleOAuthSetupSuccess}
    onCancel={() => (showGoogleOAuthSetup = false)} />
