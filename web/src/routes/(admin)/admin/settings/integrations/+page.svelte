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
    import slackLogo from '$lib/images/icons/slack.svg'
    import atlassianLogo from '$lib/images/icons/atlassian.svg'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'
    import confluenceLogo from '$lib/images/icons/confluence.svg'
    import jiraLogo from '$lib/images/icons/jira.svg'
    import { Globe, HardDrive, Loader2 } from '@lucide/svelte'
    import GoogleWorkspaceSetup from '$lib/components/google-workspace-setup.svelte'
    import AtlassianConnectorSetup from '$lib/components/atlassian-connector-setup.svelte'
    import WebConnectorSetupDialog from '$lib/components/web-connector-setup-dialog.svelte'
    import { SourceType } from '$lib/types'
    import { onMount, onDestroy } from 'svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { data }: PageProps = $props()

    let runningSyncs = $state<Map<string, SyncRun>>(data.runningSyncs)
    let eventSource = $state<EventSource | null>(null)

    onMount(() => {
        // Set up Server-Sent Events for real-time sync status updates
        eventSource = new EventSource('/api/indexing/status')
        eventSource.onmessage = (event) => {
            try {
                const statusData = JSON.parse(event.data)
                if (statusData.overall?.latestSyncRuns) {
                    // Update running syncs from SSE data
                    const newRunningSyncs = new Map<string, SyncRun>()
                    statusData.overall.latestSyncRuns.forEach((sync: any) => {
                        if (sync.status === 'running') {
                            newRunningSyncs.set(sync.sourceId, sync)
                        }
                    })
                    runningSyncs = newRunningSyncs
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
                console.error('Failed to trigger sync')
            }
            // SSE will handle the real-time update
        } catch (error) {
            console.error('Error triggering sync:', error)
        }
    }

    let showGoogleSetup = $state(false)
    let showAtlassianSetup = $state(false)
    let showWebSetup = $state(false)

    function handleConnect(integrationId: string) {
        if (integrationId === 'google') {
            showGoogleSetup = true
        } else if (integrationId === 'atlassian') {
            showAtlassianSetup = true
        } else if (integrationId === 'web') {
            showWebSetup = true
        }
        // TODO: Handle filesystem integration
    }

    function handleGoogleSetupSuccess() {
        showGoogleSetup = false
        window.location.reload()
    }

    function handleAtlassianSetupSuccess() {
        showAtlassianSetup = false
        window.location.reload()
    }

    function handleWebSetupSuccess() {
        showWebSetup = false
        window.location.reload()
    }

    function getSourceIcon(sourceType: SourceType) {
        switch (sourceType) {
            case SourceType.GOOGLE_DRIVE:
                return googleDriveLogo
            case SourceType.GMAIL:
                return gmailLogo
            case SourceType.SLACK:
                return slackLogo
            case SourceType.CONFLUENCE:
                return confluenceLogo
            case SourceType.JIRA:
                return jiraLogo
            case SourceType.WEB:
                return null
            case SourceType.LOCAL_FILES:
                return null
            default:
                return null
        }
    }

    function getIntegrationIcon(integrationId: string) {
        switch (integrationId) {
            case 'google':
                return googleLogo
            case 'slack':
                return slackLogo
            case 'atlassian':
                return atlassianLogo
            default:
                return null
        }
    }

    function formatDate(date: Date | null) {
        if (!date) return 'Never'
        return new Date(date).toLocaleDateString()
    }

    function getStatusColor(isActive: boolean) {
        return isActive
            ? 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
            : 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-300'
    }

    function getConfigureUrl(sourceType: SourceType, sourceId: string): string {
        switch (sourceType) {
            case SourceType.GOOGLE_DRIVE:
                return `/admin/settings/integrations/drive/${sourceId}`
            case SourceType.GMAIL:
                return `/admin/settings/integrations/gmail/${sourceId}`
            case SourceType.CONFLUENCE:
                return `/admin/settings/integrations/confluence/${sourceId}`
            case SourceType.JIRA:
                return `/admin/settings/integrations/jira/${sourceId}`
            case SourceType.SLACK:
                return `/admin/settings/integrations/slack/${sourceId}`
            case SourceType.WEB:
                return `/admin/settings/integrations/web/${sourceId}`
            case SourceType.LOCAL_FILES:
                return `/admin/settings/integrations/filesystem/${sourceId}`
            default:
                return '#'
        }
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
            <p class="text-muted-foreground mt-2">Manage your data source connections</p>
        </div>

        <!-- Connected Sources Section -->
        <div class="space-y-4">
            <div>
                <h2 class="text-xl font-semibold">Connected Sources</h2>
                <p class="text-muted-foreground text-sm">Active data sources syncing with Omni</p>
            </div>

            {#if data.connectedSources.length > 0}
                <div class="space-y-2">
                    {#each data.connectedSources as source}
                        <div
                            class="flex items-center justify-between gap-4 rounded-lg border px-4 py-3">
                            <div class="flex flex-1 items-start gap-3">
                                {#if getSourceIcon(source.sourceType)}
                                    <img
                                        src={getSourceIcon(source.sourceType)}
                                        alt={source.name}
                                        class="h-6 w-6" />
                                {:else if source.sourceType === 'web'}
                                    <Globe class="h-6 w-6" />
                                {:else if source.sourceType === 'local_files'}
                                    <HardDrive class="h-6 w-6" />
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
                                        class="text-muted-foreground flex items-center gap-2 text-xs">
                                        {#if runningSyncs.has(source.id)}
                                            <span
                                                >Syncing now... ({runningSyncs.get(source.id)
                                                    ?.documentsProcessed ?? 0} documents processed)</span>
                                        {:else}
                                            <span>Last sync: {formatDate(source.lastSyncAt)}</span>
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
                                        disabled={runningSyncs.has(source.id)}
                                        onclick={() => handleSync(source.id)}>
                                        Sync
                                    </Button>
                                {/if}
                                <Button
                                    variant="outline"
                                    size="sm"
                                    class="cursor-pointer"
                                    href={getConfigureUrl(
                                        source.sourceType as SourceType,
                                        source.id,
                                    )}>
                                    Configure
                                </Button>
                            </div>
                        </div>
                    {/each}
                </div>
            {:else}
                <div class="py-12 text-center">
                    <p class="text-muted-foreground text-sm">
                        No connected sources yet. Connect an integration below to get started.
                    </p>
                </div>
            {/if}
        </div>

        <!-- Available Integrations Section -->
        <div class="space-y-4">
            <div>
                <h2 class="text-xl font-semibold">Available Integrations</h2>
                <p class="text-muted-foreground text-sm">
                    Connect new data sources to search across them and take action.
                </p>
            </div>

            <div class="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
                {#each data.availableIntegrations as integration}
                    <Card class="flex flex-col">
                        <CardHeader>
                            <CardTitle class="flex items-center gap-3">
                                {#if getIntegrationIcon(integration.id)}
                                    <img
                                        src={getIntegrationIcon(integration.id)}
                                        alt={integration.name}
                                        class="h-6 w-6" />
                                {:else if integration.id === 'web'}
                                    <Globe class="h-6 w-6" />
                                {:else if integration.id === 'filesystem'}
                                    <HardDrive class="h-6 w-6" />
                                {/if}
                                <span>{integration.name}</span>
                            </CardTitle>
                            <CardDescription>{integration.description}</CardDescription>
                        </CardHeader>
                        <CardContent class="flex-1" />
                        <CardFooter>
                            {#if integration.id === 'slack' || integration.id === 'filesystem'}
                                <Button size="sm" disabled>Coming Soon</Button>
                            {:else}
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => handleConnect(integration.id)}>
                                    Connect
                                </Button>
                            {/if}
                        </CardFooter>
                    </Card>
                {/each}
            </div>
        </div>
    </div>
</div>

<GoogleWorkspaceSetup
    bind:open={showGoogleSetup}
    onSuccess={handleGoogleSetupSuccess}
    onCancel={() => (showGoogleSetup = false)} />

<AtlassianConnectorSetup
    bind:open={showAtlassianSetup}
    onSuccess={handleAtlassianSetupSuccess}
    onCancel={() => (showAtlassianSetup = false)} />

<WebConnectorSetupDialog
    bind:open={showWebSetup}
    onSuccess={handleWebSetupSuccess}
    onCancel={() => (showWebSetup = false)} />
