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
    import * as ButtonGroup from '$lib/components/ui/button-group/index.js'
    import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js'
    import * as Popover from '$lib/components/ui/popover'
    import * as Alert from '$lib/components/ui/alert'
    import * as Tabs from '$lib/components/ui/tabs'
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
    import nextcloudLogo from '$lib/images/icons/nextcloud.svg'
    import paperlessLogo from '$lib/images/icons/paperless.svg'
    import imapLogo from '$lib/images/icons/imap.svg'
    import { copyTextToClipboard } from '$lib/utils'
    import { getSourceIconPath } from '$lib/utils/icons'
    import {
        AlertTriangle,
        Check,
        ChevronDown,
        Cloud,
        Copy,
        Globe,
        HardDrive,
        KeyRound,
        Mail,
    } from '@lucide/svelte'
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
    import PaperlessConnectorSetup from '$lib/components/paperless-connector-setup.svelte'
    import NextcloudConnectorSetup from '$lib/components/nextcloud-connector-setup.svelte'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'
    import { Badge } from '$lib/components/ui/badge'
    import { SourceType } from '$lib/types'
    import { formatDate, getSourceNoun, getStatusColor } from '$lib/utils/sources'
    import { invalidateAll } from '$app/navigation'
    import { page } from '$app/state'
    import { onMount, onDestroy } from 'svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { data }: PageProps = $props()

    type SourceId = string
    type SyncStatusPayload = {
        overall?: {
            latestSyncRuns?: SyncRun[]
            documentCounts?: Record<SourceId, number>
        }
    }

    type OAuthProvider = (typeof data.oauthProviders)[number]

    let latestSyncRuns = $state<Map<SourceId, SyncRun>>(data.latestSyncRuns)
    let sourceHealth = $state<Map<SourceId, 'healthy' | 'unhealthy'>>(data.sourceHealth)
    let documentCounts = $state<Record<SourceId, number>>({})
    let eventSource = $state<EventSource | null>(null)
    let activeTab = $state(page.url.searchParams.get('tab') === 'oauth' ? 'oauth' : 'sources')
    let activeOAuthProvider = $state<OAuthProvider | null>(null)
    let redirectUriCopied = $state(false)
    let copyResetTimer: ReturnType<typeof setTimeout> | null = null

    $effect(() => {
        latestSyncRuns = data.latestSyncRuns
        sourceHealth = data.sourceHealth
    })

    onMount(() => {
        // Set up Server-Sent Events for real-time sync status updates
        eventSource = new EventSource('/api/indexing/status?scope=org')
        eventSource.onmessage = (event) => {
            try {
                const statusData = JSON.parse(event.data) as SyncStatusPayload
                if (statusData.overall?.latestSyncRuns) {
                    const updated = new Map(latestSyncRuns)
                    for (const sync of statusData.overall.latestSyncRuns) {
                        updated.set(sync.sourceId, sync)
                    }
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

    type SyncMode = 'incremental' | 'full'

    async function handleSync(sourceId: string, mode: SyncMode = 'incremental') {
        try {
            const response = await fetch(`/api/sources/${sourceId}/sync`, {
                method: 'POST',
                headers: {
                    'content-type': 'application/json',
                },
                body: JSON.stringify({ sync_mode: mode }),
            })
            if (!response.ok) {
                const body = await response.json().catch(() => null)
                toast.error(body?.message || 'Failed to trigger sync')
            } else {
                toast.success(
                    `${mode === 'full' ? 'Full sync' : 'Incremental sync'} triggered successfully`,
                )
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

    function closeOAuthDialog() {
        activeOAuthProvider = null
    }

    async function copyRedirectUri() {
        try {
            await copyTextToClipboard(data.oauthRedirectUri)
            redirectUriCopied = true
            toast.success('Redirect URI copied')
            if (copyResetTimer) clearTimeout(copyResetTimer)
            copyResetTimer = setTimeout(() => {
                redirectUriCopied = false
                copyResetTimer = null
            }, 2000)
        } catch {
            toast.error('Failed to copy redirect URI')
        }
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
        nextcloud: nextcloudLogo,
        paperless_ngx: paperlessLogo,
        imap: imapLogo,
    }

    const oauthProviderIcons: Record<string, string> = {
        atlassian: atlassianLogo,
        clickup: clickupLogo,
        fireflies: firefliesLogo,
        github: githubLogo,
        google: googleLogo,
        hubspot: hubspotLogo,
        linear: linearLogo,
        microsoft: microsoftLogo,
        nextcloud: nextcloudLogo,
        notion: notionLogo,
        slack: slackLogo,
    }

    function getIntegrationIcon(integrationId: string): string | null {
        return integrationIcons[integrationId] ?? null
    }

    function normalizeStatus(status: string | null | undefined) {
        return status?.toLowerCase()
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
            <p class="text-muted-foreground mt-2">
                Manage organization-level data source connections
            </p>
        </div>

        <Tabs.Root bind:value={activeTab}>
            <Tabs.List>
                <Tabs.Trigger
                    value="sources"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm">
                    Sources
                </Tabs.Trigger>
                <Tabs.Trigger
                    value="oauth"
                    class="data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm">
                    OAuth Apps
                </Tabs.Trigger>
            </Tabs.List>

            <Tabs.Content value="sources" class="space-y-8">
                <!-- Connected Sources Section -->
                <div class="space-y-4">
                    <div>
                        <h2 class="text-xl font-semibold">Organization Integrations</h2>
                        <p class="text-muted-foreground text-sm">
                            Org-level data sources syncing with Omni
                        </p>
                    </div>

                    {#if data.connectedSources.length > 0}
                        <div class="space-y-2">
                            {#each data.connectedSources as source}
                                {@const noun = getSourceNoun(source.sourceType as SourceType)}
                                {@const sync = latestSyncRuns.get(source.id)}
                                {@const health = sourceHealth.get(source.id)}
                                {@const isSyncRunning = normalizeStatus(sync?.status) === 'running'}
                                <div
                                    class="bg-card flex items-center justify-between gap-4 rounded-lg border px-4 py-3">
                                    <div class="flex flex-1 items-start gap-3">
                                        {#if getSourceIconPath(source.sourceType as SourceType)}
                                            <img
                                                src={getSourceIconPath(
                                                    source.sourceType as SourceType,
                                                )}
                                                alt={source.name}
                                                class="h-6 w-6" />
                                        {:else if source.sourceType === 'web'}
                                            <Globe class="h-6 w-6" />
                                        {:else if source.sourceType === 'local_files'}
                                            <HardDrive class="h-6 w-6" />
                                        {:else if source.sourceType === 'imap'}
                                            <Mail class="h-6 w-6" />
                                        {:else if source.sourceType === 'paperless_ngx'}
                                            <HardDrive class="h-6 w-6" />
                                        {:else if source.sourceType === 'nextcloud'}
                                            <Cloud class="h-6 w-6" />
                                        {/if}
                                        <div class="flex flex-col gap-0.5">
                                            <div class="flex items-center gap-2">
                                                <span class="truncate overflow-hidden font-medium"
                                                    >{source.name}</span>
                                                <span
                                                    class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(source.isActive)}`}>
                                                    {source.isActive ? 'Enabled' : 'Disabled'}
                                                </span>
                                                {#if health === 'unhealthy'}
                                                    <span
                                                        class="inline-flex items-center gap-1 rounded-full bg-red-100 px-2 py-0.5 text-xs font-medium text-red-800 dark:bg-red-900/20 dark:text-red-400">
                                                        <AlertTriangle class="h-3 w-3" />
                                                        Unhealthy
                                                    </span>
                                                    <Popover.Root>
                                                        <Popover.Trigger
                                                            class="cursor-pointer border-0 bg-transparent p-0 text-xs font-medium text-red-600 underline-offset-2 hover:underline">
                                                            View error
                                                        </Popover.Trigger>
                                                        <Popover.Content
                                                            align="start"
                                                            class="bg-card w-96 max-w-[calc(100vw-2rem)] p-4">
                                                            <Alert.Root
                                                                class="border-0 bg-transparent p-0 text-red-900 dark:text-red-50">
                                                                <AlertTriangle class="h-4 w-4" />
                                                                <Alert.Title
                                                                    >Source unhealthy</Alert.Title>
                                                                <Alert.Description>
                                                                    Scheduled syncs have been paused
                                                                    after repeated failures.
                                                                    {#if sync?.errorMessage}
                                                                        <div class="mt-3 space-y-1">
                                                                            <div
                                                                                class="text-xs font-medium">
                                                                                Error message
                                                                            </div>
                                                                            <code
                                                                                class="block max-h-48 overflow-y-auto font-mono text-xs break-words whitespace-pre-wrap">
                                                                                {sync.errorMessage}
                                                                            </code>
                                                                        </div>
                                                                    {/if}
                                                                </Alert.Description>
                                                            </Alert.Root>
                                                        </Popover.Content>
                                                    </Popover.Root>
                                                {/if}
                                            </div>
                                            <div
                                                class="text-muted-foreground flex items-center gap-1 text-xs">
                                                {#if sync && normalizeStatus(sync.status) === 'running'}
                                                    {#if sync.documentsScanned && sync.documentsScanned > 0}
                                                        <span
                                                            >Syncing... {sync.documentsScanned.toLocaleString()}
                                                            {noun} scanned{#if sync.documentsUpdated && sync.documentsUpdated > 0},
                                                                {sync.documentsUpdated.toLocaleString()}
                                                                updated{/if}
                                                            {#if documentCounts[source.id]}
                                                                ({documentCounts[
                                                                    source.id
                                                                ].toLocaleString()} indexed, scanned includes
                                                                duplicates across users)
                                                            {/if}</span>
                                                    {:else}
                                                        <span>Syncing...</span>
                                                    {/if}
                                                {:else}
                                                    <span
                                                        >Last sync: {formatDate(
                                                            sync?.completedAt ?? null,
                                                        )}</span>
                                                {/if}
                                                {#if !sync || normalizeStatus(sync.status) !== 'running'}
                                                    {#if documentCounts[source.id]}
                                                        <span class="text-muted-foreground">·</span>
                                                        <span
                                                            >{documentCounts[
                                                                source.id
                                                            ].toLocaleString()}
                                                            {noun} indexed</span>
                                                    {/if}
                                                {/if}
                                            </div>
                                        </div>
                                    </div>
                                    <div class="flex gap-2">
                                        {#if source.isActive}
                                            <ButtonGroup.Root>
                                                <Button
                                                    variant="default"
                                                    size="sm"
                                                    class="cursor-pointer"
                                                    disabled={isSyncRunning}
                                                    onclick={() => handleSync(source.id)}>
                                                    Sync
                                                </Button>
                                                <DropdownMenu.Root>
                                                    <DropdownMenu.Trigger>
                                                        {#snippet child({ props })}
                                                            <Button
                                                                {...props}
                                                                variant="default"
                                                                size="sm"
                                                                class="cursor-pointer px-2"
                                                                disabled={isSyncRunning}
                                                                aria-label="Choose sync mode">
                                                                <ChevronDown class="h-4 w-4" />
                                                            </Button>
                                                        {/snippet}
                                                    </DropdownMenu.Trigger>
                                                    <DropdownMenu.Content align="end" class="w-56">
                                                        <DropdownMenu.Item
                                                            disabled={isSyncRunning}
                                                            onSelect={() =>
                                                                handleSync(
                                                                    source.id,
                                                                    'incremental',
                                                                )}
                                                            class="cursor-pointer items-start">
                                                            <Check
                                                                class="mt-0.5 h-4 w-4 text-green-600" />
                                                            <div class="flex flex-col gap-0.5">
                                                                <span>Run incremental sync</span>
                                                                <span
                                                                    class="text-muted-foreground text-xs">
                                                                    Default / recommended
                                                                </span>
                                                            </div>
                                                        </DropdownMenu.Item>
                                                        <DropdownMenu.Item
                                                            disabled={isSyncRunning}
                                                            onSelect={() =>
                                                                handleSync(source.id, 'full')}
                                                            class="cursor-pointer items-start">
                                                            <div class="h-4 w-4 shrink-0"></div>
                                                            <div class="flex flex-col gap-0.5">
                                                                <span>Run full sync</span>
                                                                <span
                                                                    class="text-muted-foreground text-xs">
                                                                    Re-scan all data
                                                                </span>
                                                            </div>
                                                        </DropdownMenu.Item>
                                                    </DropdownMenu.Content>
                                                </DropdownMenu.Root>
                                            </ButtonGroup.Root>
                                        {/if}
                                        <Button
                                            variant="ghost"
                                            size="sm"
                                            class="cursor-pointer"
                                            href={getConfigureUrl(
                                                source.sourceType as SourceType,
                                                source.id,
                                            )}>
                                            Settings
                                        </Button>
                                    </div>
                                </div>
                            {/each}
                        </div>
                    {:else}
                        <div class="py-12 text-center">
                            <p class="text-muted-foreground text-sm">
                                No org-level integrations configured yet. Connect an integration
                                below to get started.
                            </p>
                        </div>
                    {/if}
                </div>

                <!-- Available Integrations Section -->
                <div class="space-y-4">
                    <div>
                        <h2 class="text-xl font-semibold">Available Integrations</h2>
                        <p class="text-muted-foreground text-sm">
                            Connect apps at the organization level to sync shared data with Omni
                        </p>
                    </div>

                    <div class="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
                        {#each data.availableIntegrations as integration}
                            <Card class="flex flex-col">
                                <CardHeader>
                                    <CardTitle class="flex items-center gap-3">
                                        {#if getIntegrationIcon(integration.id)}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <img
                                                    src={getIntegrationIcon(integration.id)}
                                                    alt={integration.name}
                                                    class="h-6 w-6 object-contain" />
                                            </div>
                                        {:else if integration.id === 'web'}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <Globe class="h-6 w-6 text-slate-700" />
                                            </div>
                                        {:else if integration.id === 'filesystem'}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <HardDrive class="h-6 w-6 text-slate-700" />
                                            </div>
                                        {:else if integration.id === 'imap'}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <Mail class="h-6 w-6 text-slate-700" />
                                            </div>
                                        {:else if integration.id === 'paperless_ngx'}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <HardDrive class="h-6 w-6 text-slate-700" />
                                            </div>
                                        {:else if integration.id === 'nextcloud'}
                                            <div
                                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm">
                                                <Cloud class="h-6 w-6 text-slate-700" />
                                            </div>
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
            </Tabs.Content>

            <Tabs.Content value="oauth" class="space-y-6">
                <div>
                    <h2 class="text-xl font-semibold">OAuth app clients</h2>
                    <p class="text-muted-foreground text-sm">
                        Configure OAuth clients for your connected apps. These clients are used when
                        users set up their own connections, or when users invoke API calls to
                        connected apps.
                    </p>
                </div>

                <div class="space-y-6">
                    {#if data.oauthProviders.length > 0}
                        <div class="overflow-hidden rounded-lg border">
                            <div
                                class="bg-muted/50 text-muted-foreground grid grid-cols-[1.4fr_0.8fr_1fr_0.8fr] gap-4 px-4 py-3 text-sm font-medium">
                                <div>Provider</div>
                                <div>Status</div>
                                <div>Last updated</div>
                                <div class="text-right"></div>
                            </div>
                            {#each data.oauthProviders as provider}
                                <div
                                    class="grid grid-cols-[1.4fr_0.8fr_1fr_0.8fr] items-center gap-4 border-t px-4 py-3 text-sm">
                                    <div class="flex items-center gap-2 font-medium">
                                        {#if oauthProviderIcons[provider.provider]}
                                            <img
                                                src={oauthProviderIcons[provider.provider]}
                                                alt={provider.displayName}
                                                class="h-5 w-5 shrink-0 object-contain" />
                                        {:else}
                                            <KeyRound
                                                class="text-muted-foreground h-5 w-5 shrink-0" />
                                        {/if}
                                        {provider.displayName}
                                    </div>
                                    <div>
                                        {#if provider.configured}
                                            <Badge variant="secondary">Configured</Badge>
                                        {:else}
                                            <Badge variant="outline">Not configured</Badge>
                                        {/if}
                                    </div>
                                    <div class="text-muted-foreground">
                                        {formatDate(provider.updatedAt)}
                                    </div>
                                    <div class="text-right">
                                        <Button
                                            size="sm"
                                            variant={provider.configured ? 'outline' : 'default'}
                                            class="cursor-pointer"
                                            onclick={() => (activeOAuthProvider = provider)}>
                                            {provider.configured ? 'Edit' : 'Add client'}
                                        </Button>
                                    </div>
                                </div>
                            {/each}
                        </div>
                        <div class="space-y-2">
                            <div class="text-sm font-medium">Shared redirect URI</div>
                            <p class="text-muted-foreground text-sm">
                                Use this callback URL when creating OAuth clients for your
                                providers.
                            </p>
                            <div class="flex gap-2">
                                <code
                                    class="bg-muted text-muted-foreground flex-1 rounded-md px-3 py-2 text-sm break-all">
                                    {data.oauthRedirectUri}
                                </code>
                                <Button
                                    variant="outline"
                                    class="cursor-pointer"
                                    onclick={copyRedirectUri}>
                                    {#if redirectUriCopied}
                                        <Check class="h-4 w-4 text-green-600" />
                                        Copied
                                    {:else}
                                        <Copy class="h-4 w-4" />
                                        Copy
                                    {/if}
                                </Button>
                            </div>
                        </div>
                    {:else}
                        <div class="py-12 text-center">
                            <p class="text-muted-foreground text-sm">
                                No OAuth-capable connector manifests are currently registered.
                            </p>
                        </div>
                    {/if}
                </div>
            </Tabs.Content>
        </Tabs.Root>
    </div>
</div>

<GoogleWorkspaceSetup
    open={activeSetup === 'google'}
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

<PaperlessConnectorSetup
    open={activeSetup === 'paperless_ngx'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

<NextcloudConnectorSetup
    open={activeSetup === 'nextcloud'}
    onSuccess={handleSetupSuccess}
    onCancel={closeSetup} />

{#if activeOAuthProvider}
    <OAuthClientConfigDialog
        open={activeOAuthProvider !== null}
        provider={activeOAuthProvider.provider}
        displayName={activeOAuthProvider.displayName}
        configured={activeOAuthProvider.configured}
        config={activeOAuthProvider.config}
        onSaved={closeOAuthDialog}
        onCancel={closeOAuthDialog} />
{/if}
