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
    import hubspotLogo from '$lib/images/icons/hubspot.svg'
    import firefliesLogo from '$lib/images/icons/fireflies.svg'
    import microsoftLogo from '$lib/images/icons/microsoft.svg'
    import clickupLogo from '$lib/images/icons/clickup.svg'
    import notionLogo from '$lib/images/icons/notion.svg'
    import linearLogo from '$lib/images/icons/linear.svg'
    import githubLogo from '$lib/images/icons/github.svg'
    import { getSourceIconPath } from '$lib/utils/icons'
    import { Globe, HardDrive, Mail } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import GoogleWorkspaceSetup from '$lib/components/google-workspace-setup.svelte'
    import AtlassianConnectorSetup from '$lib/components/atlassian-connector-setup.svelte'
    import SlackConnectorSetup from '$lib/components/slack-connector-setup.svelte'
    import HubspotConnectorSetup from '$lib/components/hubspot-connector-setup.svelte'
    import FirefliesConnectorSetup from '$lib/components/fireflies-connector-setup.svelte'
    import ImapConnectorSetup from '$lib/components/imap-connector-setup.svelte'
    import MicrosoftConnectorSetup from '$lib/components/microsoft-connector-setup.svelte'
    import WebConnectorSetupDialog from '$lib/components/web-connector-setup-dialog.svelte'
    import FilesystemConnectorSetupDialog from '$lib/components/filesystem-connector-setup-dialog.svelte'
    import ClickupConnectorSetup from '$lib/components/clickup-connector-setup.svelte'
    import NotionConnectorSetup from '$lib/components/notion-connector-setup.svelte'
    import LinearConnectorSetup from '$lib/components/linear-connector-setup.svelte'
    import GithubConnectorSetup from '$lib/components/github-connector-setup.svelte'
    import { SourceType } from '$lib/types'
    import { invalidateAll } from '$app/navigation'
    import { onMount, onDestroy } from 'svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { data }: PageProps = $props()

    type SourceId = string
    let latestSyncRuns = $state<Map<SourceId, SyncRun>>(data.latestSyncRuns)
    let documentCounts = $state<Record<SourceId, number>>({})
    let eventSource = $state<EventSource | null>(null)

    $effect(() => {
        latestSyncRuns = data.latestSyncRuns
    })

    onMount(() => {
        // Set up Server-Sent Events for real-time sync status updates
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

    let activeSetup = $state<string | null>(null)

    function handleConnect(integrationId: string) {
        activeSetup = integrationId
    }

    function handleSetupSuccess() {
        activeSetup = null
        window.location.reload()
    }

    function closeSetup() {
        activeSetup = null
    }

    const integrationIcons: Record<string, string> = {
        google: googleLogo,
        slack: slackLogo,
        atlassian: atlassianLogo,
        hubspot: hubspotLogo,
        fireflies: firefliesLogo,
        microsoft: microsoftLogo,
        clickup: clickupLogo,
        notion: notionLogo,
        linear: linearLogo,
        github: githubLogo,
    }

    function getIntegrationIcon(integrationId: string): string | null {
        return integrationIcons[integrationId] ?? null
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

    const sourceNouns: Record<string, string> = {
        [SourceType.GOOGLE_DRIVE]: 'documents',
        [SourceType.GMAIL]: 'threads',
        [SourceType.SLACK]: 'messages',
        [SourceType.CONFLUENCE]: 'pages',
        [SourceType.JIRA]: 'issues',
        [SourceType.HUBSPOT]: 'records',
        [SourceType.FIREFLIES]: 'transcripts',
        [SourceType.IMAP]: 'emails',
        [SourceType.ONE_DRIVE]: 'files',
        [SourceType.OUTLOOK]: 'emails',
        [SourceType.OUTLOOK_CALENDAR]: 'events',
        [SourceType.SHARE_POINT]: 'documents',
        [SourceType.WEB]: 'pages',
        [SourceType.LINEAR]: 'items',
        [SourceType.LOCAL_FILES]: 'files',
        [SourceType.CLICKUP]: 'tasks',
        [SourceType.NOTION]: 'pages',
        [SourceType.GITHUB]: 'documents',
    }

    function getSourceNoun(sourceType: SourceType): string {
        return sourceNouns[sourceType] ?? 'documents'
    }

    const sourceTypeSlug: Record<string, string> = {
        [SourceType.GOOGLE_DRIVE]: 'drive',
        [SourceType.GMAIL]: 'gmail',
        [SourceType.LOCAL_FILES]: 'filesystem',
        [SourceType.ONE_DRIVE]: 'microsoft',
        [SourceType.OUTLOOK]: 'microsoft',
        [SourceType.OUTLOOK_CALENDAR]: 'microsoft',
        [SourceType.SHARE_POINT]: 'microsoft',
        [SourceType.MS_TEAMS]: 'microsoft',
    }

    function getConfigureUrl(sourceType: SourceType, sourceId: string): string {
        const slug = sourceTypeSlug[sourceType] ?? sourceType
        return `/admin/settings/integrations/${slug}/${sourceId}`
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
                        {@const noun = getSourceNoun(source.sourceType as SourceType)}
                        {@const sync = latestSyncRuns.get(source.id)}
                        <div
                            class="flex items-center justify-between gap-4 rounded-lg border px-4 py-3">
                            <div class="flex flex-1 items-start gap-3">
                                {#if getSourceIconPath(source.sourceType as SourceType)}
                                    <img
                                        src={getSourceIconPath(source.sourceType as SourceType)}
                                        alt={source.name}
                                        class="h-6 w-6" />
                                {:else if source.sourceType === 'web'}
                                    <Globe class="h-6 w-6" />
                                {:else if source.sourceType === 'local_files'}
                                    <HardDrive class="h-6 w-6" />
                                {:else if source.sourceType === 'imap'}
                                    <Mail class="h-6 w-6" />
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
                                            <span class="text-muted-foreground">·</span>
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
                                {:else if integration.id === 'imap'}
                                    <Mail class="h-6 w-6" />
                                {/if}
                                <span>{integration.name}</span>
                            </CardTitle>
                            <CardDescription>{integration.description}</CardDescription>
                        </CardHeader>
                        <CardContent class="flex-1" />
                        <CardFooter class="flex gap-2">
                            <Button
                                size="sm"
                                class="cursor-pointer"
                                onclick={() => handleConnect(integration.id)}>
                                Connect
                            </Button>
                        </CardFooter>
                    </Card>
                {/each}
            </div>
        </div>
    </div>
</div>

<GoogleWorkspaceSetup
    open={activeSetup === 'google'}
    googleOAuthConfigured={data.googleOAuthConfigured}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<AtlassianConnectorSetup
    open={activeSetup === 'atlassian'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<SlackConnectorSetup
    open={activeSetup === 'slack'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<WebConnectorSetupDialog
    open={activeSetup === 'web'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<FilesystemConnectorSetupDialog
    open={activeSetup === 'filesystem'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<HubspotConnectorSetup
    open={activeSetup === 'hubspot'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<FirefliesConnectorSetup
    open={activeSetup === 'fireflies'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<ImapConnectorSetup
    open={activeSetup === 'imap'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<MicrosoftConnectorSetup
    open={activeSetup === 'microsoft'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<ClickupConnectorSetup
    open={activeSetup === 'clickup'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<NotionConnectorSetup
    open={activeSetup === 'notion'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<LinearConnectorSetup
    open={activeSetup === 'linear'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<GithubConnectorSetup
    open={activeSetup === 'github'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />
