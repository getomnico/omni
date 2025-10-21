<script lang="ts">
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import { Button, buttonVariants } from '$lib/components/ui/button'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card'
    import * as Dialog from '$lib/components/ui/dialog'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { AuthType } from '$lib/types'
    import { onDestroy, onMount } from 'svelte'
    import { toast } from 'svelte-sonner'
    import { goto } from '$app/navigation'
    import type { PageProps } from './$types'
    import googleLogo from '$lib/images/icons/google.svg'
    import slackLogo from '$lib/images/icons/slack.svg'
    import atlassianLogo from '$lib/images/icons/atlassian.svg'
    import googleDriveLogo from '$lib/images/icons/google-drive.svg'
    import gmailLogo from '$lib/images/icons/gmail.svg'

    let { data }: PageProps = $props()

    type SyncStatus = {
        status: string
        syncType: string
        documentsProcessed: number
        documentsUpdated: number
        startedAt: Date
        completedAt: Date | null
        errorMessage: string | null
    }

    let latestSyncRuns = $state<any[]>(data.latestSyncRuns || [])
    let overallIndexingStats = $state({
        totalDocumentsIndexed: data.documentStats?.totalDocumentsIndexed || 0,
        documentsBySource: data.documentStats?.documentsBySource || ({} as Record<string, number>),
    })
    let eventSource = $state<EventSource | null>(null)
    let disconnectingSourceId = $state<string | null>(null)
    let syncingSourceId = $state<string | null>(null)

    // Service account setup state
    let showSetupDialog = $state(false)
    let selectedIntegration = $state<any>(null)
    let serviceAccountJson = $state('')
    let apiToken = $state('')
    let principalEmail = $state('')
    let domain = $state('')
    let isSubmitting = $state(false)

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }

    function getSourceByType(providerId: string) {
        if (providerId === 'google') {
            // Check if we have both Google Drive and Gmail sources
            const driveSource = data.connectedSources.find(
                (source) => source.sourceType === 'google_drive',
            )
            const gmailSource = data.connectedSources.find(
                (source) => source.sourceType === 'gmail',
            )
            // Return true if we have at least one Google source
            return driveSource || gmailSource
        } else if (providerId === 'atlassian') {
            return data.connectedSources.find(
                (source) => source.sourceType === 'confluence' || source.sourceType === 'jira',
            )
        }
        return data.connectedSources.find((source) => source.sourceType === providerId)
    }

    function getLatestSyncForSource(sourceId: string): SyncStatus | null {
        const latestSync = latestSyncRuns.find((sync) => sync.sourceId === sourceId)
        return latestSync || null
    }

    function getStatusColor(status: string) {
        switch (status) {
            case 'running':
                return 'bg-blue-100 text-blue-800 dark:bg-blue-900/20 dark:text-blue-400'
            case 'completed':
                return 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
            case 'failed':
                return 'bg-red-100 text-red-800 dark:bg-red-900/20 dark:text-red-400'
            case 'pending':
                return 'bg-yellow-100 text-yellow-800 dark:bg-yellow-900/20 dark:text-yellow-400'
            default:
                return 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-300'
        }
    }

    function openSetupDialog(integration: any) {
        selectedIntegration = integration
        showSetupDialog = true
        serviceAccountJson = ''
        apiToken = ''
        principalEmail = ''
        domain = ''
    }

    async function setupServiceAccount() {
        if (!selectedIntegration) return

        isSubmitting = true
        try {
            let credentials: any = {}
            let config: any = {}
            let provider: string = selectedIntegration.id
            let authType: string = ''

            if (selectedIntegration.id === 'google') {
                if (!serviceAccountJson.trim()) {
                    throw new Error('Service account JSON is required')
                }

                if (!principalEmail.trim()) {
                    throw new Error('Admin email is required')
                }

                if (!domain.trim()) {
                    throw new Error('Organization domain is required')
                }

                // Validate JSON
                try {
                    JSON.parse(serviceAccountJson)
                } catch {
                    throw new Error('Invalid JSON format')
                }

                credentials = { service_account_key: serviceAccountJson }
                config = {
                    // Note: Scopes are now determined dynamically by the connector based on source type
                    // We don't set scopes here anymore - the connector will determine the appropriate scopes
                    domain: domain || null,
                }
                authType = AuthType.JWT
            } else if (selectedIntegration.id === 'atlassian') {
                if (!principalEmail.trim() || !apiToken.trim()) {
                    throw new Error('Email and API token are required')
                }

                credentials = { api_key: apiToken }
                config = {}
                authType = AuthType.API_KEY
            } else if (selectedIntegration.id === 'slack') {
                if (!apiToken.trim()) {
                    throw new Error('Bot token is required')
                }

                credentials = { bot_token: apiToken }
                config = {}
                authType = AuthType.BOT_TOKEN
            }

            // For Google, create separate sources for Drive and Gmail
            if (selectedIntegration.id === 'google') {
                // Create Google Drive source
                const driveSourceResponse = await fetch('/api/sources', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: 'Google Drive',
                        sourceType: 'google_drive',
                        config: {},
                    }),
                })

                if (!driveSourceResponse.ok) {
                    throw new Error('Failed to create Google Drive source')
                }

                const driveSource = await driveSourceResponse.json()

                // Create service credentials for Drive source
                const driveCredentialsResponse = await fetch('/api/service-credentials', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sourceId: driveSource.id,
                        provider: provider,
                        authType: authType,
                        principalEmail: principalEmail || null,
                        credentials: credentials,
                        config: config,
                    }),
                })

                if (!driveCredentialsResponse.ok) {
                    throw new Error('Failed to create Google Drive service credentials')
                }

                // Create Gmail source
                const gmailSourceResponse = await fetch('/api/sources', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: 'Gmail',
                        sourceType: 'gmail',
                        config: {},
                    }),
                })

                if (!gmailSourceResponse.ok) {
                    throw new Error('Failed to create Gmail source')
                }

                const gmailSource = await gmailSourceResponse.json()

                // Create service credentials for Gmail source (same credentials as Drive)
                const gmailCredentialsResponse = await fetch('/api/service-credentials', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sourceId: gmailSource.id,
                        provider: provider,
                        authType: authType,
                        principalEmail: principalEmail || null,
                        credentials: credentials,
                        config: config,
                    }),
                })

                if (!gmailCredentialsResponse.ok) {
                    throw new Error('Failed to create Gmail service credentials')
                }
            } else {
                // For non-Google integrations, create single source as before
                const sourceResponse = await fetch('/api/sources', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        name: `${selectedIntegration.name} Source`,
                        sourceType: selectedIntegration.id,
                        config: {},
                    }),
                })

                if (!sourceResponse.ok) {
                    throw new Error('Failed to create source')
                }

                const source = await sourceResponse.json()

                // Then create the service credentials
                const credentialsResponse = await fetch('/api/service-credentials', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        sourceId: source.id,
                        provider: provider,
                        authType: authType,
                        principalEmail: principalEmail || null,
                        credentials: credentials,
                        config: config,
                    }),
                })

                if (!credentialsResponse.ok) {
                    throw new Error('Failed to create service credentials')
                }
            }

            toast.success(`${selectedIntegration.name} connected successfully!`)
            showSetupDialog = false

            // Redirect to configure page for Google integration
            if (selectedIntegration.id === 'google') {
                await goto('/admin/integrations/google/configure')
            } else {
                window.location.reload()
            }
        } catch (error: any) {
            console.error('Error setting up service account:', error)
            toast.error(error.message || 'Failed to set up service account')
        } finally {
            isSubmitting = false
        }
    }

    async function disconnectSource(sourceId: string) {
        disconnectingSourceId = sourceId
        try {
            const response = await fetch(`/api/sources/${sourceId}/disconnect`, {
                method: 'POST',
            })

            if (!response.ok) {
                throw new Error('Failed to disconnect source')
            }

            toast.success('Source disconnected successfully')
            window.location.reload()
        } catch (error) {
            console.error('Error disconnecting source:', error)
            toast.error('Failed to disconnect source. Please try again.')
        } finally {
            disconnectingSourceId = null
        }
    }

    async function syncSource(sourceId: string) {
        if (syncingSourceId) return

        syncingSourceId = sourceId
        try {
            const response = await fetch(`/api/sources/${sourceId}/sync`, {
                method: 'POST',
            })

            if (!response.ok) {
                throw new Error('Failed to start sync')
            }

            toast.success('Sync started successfully')
        } catch (error) {
            console.error('Error syncing source:', error)
            toast.error('Failed to start sync. Please try again.')
        } finally {
            syncingSourceId = null
        }
    }

    onMount(() => {
        // Set up Server-Sent Events for live indexing status updates
        eventSource = new EventSource('/api/indexing/status')
        eventSource.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data)

                // Update data from the new structure
                if (data.overall) {
                    // Update latest sync runs
                    if (data.overall.latestSyncRuns) {
                        latestSyncRuns = data.overall.latestSyncRuns
                    }

                    // Update document stats
                    if (data.overall.documentStats) {
                        overallIndexingStats = {
                            totalDocumentsIndexed:
                                data.overall.documentStats.totalDocumentsIndexed || 0,
                            documentsBySource: data.overall.documentStats.documentsBySource || {},
                        }
                    }
                }
            } catch (error) {
                console.error('Error parsing SSE data:', error)
            }
        }

        eventSource.onerror = (error) => {
            console.error('SSE error:', error)
        }
    })

    onDestroy(() => {
        if (eventSource) {
            eventSource.close()
        }
    })
</script>

<svelte:head>
    <title>Integrations - Omni Admin</title>
</svelte:head>

<div class="mx-auto max-w-screen-xl space-y-6 pt-8 pb-24">
    <div>
        <h1 class="text-2xl font-bold tracking-tight">Integrations</h1>
        <p class="text-muted-foreground">
            Connect Omni to your data sources using service accounts and API tokens.
        </p>
    </div>

    <!-- Overall Indexing Status -->
    {#if overallIndexingStats.totalDocumentsIndexed > 0}
        <Card>
            <CardHeader>
                <CardTitle>Document Index Statistics</CardTitle>
                <CardDescription>Total unique documents indexed across all sources</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="mb-4 text-center">
                    <div class="text-3xl font-bold">
                        {overallIndexingStats.totalDocumentsIndexed.toLocaleString()}
                    </div>
                    <div class="text-muted-foreground text-sm">Total Documents Indexed</div>
                </div>

                {#if Object.keys(overallIndexingStats.documentsBySource).length > 0}
                    <div class="mt-4 border-t pt-4">
                        <h4 class="mb-2 text-sm font-medium">Documents by Source</h4>
                        <div class="space-y-1">
                            {#each Object.entries(overallIndexingStats.documentsBySource) as [sourceId, count]}
                                {@const source = data.connectedSources.find(
                                    (s) => s.id === sourceId,
                                )}
                                {#if source}
                                    <div class="flex justify-between text-sm">
                                        <span class="text-muted-foreground">{source.name}</span>
                                        <span class="font-medium">{count.toLocaleString()}</span>
                                    </div>
                                {/if}
                            {/each}
                        </div>
                    </div>
                {/if}
            </CardContent>
        </Card>
    {/if}

    <!-- Available Integrations -->
    <div class="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
        {#each data.availableIntegrations as integration}
            {@const connectedSource = getSourceByType(integration.id)}
            <Card>
                <CardHeader>
                    <CardTitle class="flex items-center justify-between">
                        <div class="flex items-center gap-3">
                            {#if integration.id === 'google'}
                                <img src={googleLogo} alt="Google" class="h-6 w-6" />
                            {:else if integration.id === 'slack'}
                                <img src={slackLogo} alt="Slack" class="h-6 w-6" />
                            {:else if integration.id === 'atlassian'}
                                <img src={atlassianLogo} alt="Atlassian" class="h-6 w-6" />
                            {/if}
                            <span>{integration.name}</span>
                        </div>
                        {#if connectedSource}
                            <span
                                class="inline-flex items-center rounded-full bg-green-100 px-2 py-1 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                Connected
                            </span>
                        {:else}
                            <span
                                class="inline-flex items-center rounded-full bg-gray-100 px-2 py-1 text-xs font-medium text-gray-800 dark:bg-gray-800 dark:text-gray-300">
                                Not Connected
                            </span>
                        {/if}
                    </CardTitle>
                    <CardDescription>{integration.description}</CardDescription>
                </CardHeader>
                <CardContent class="flex flex-1 flex-col items-start justify-end">
                    {#if connectedSource}
                        {#if integration.id === 'google'}
                            {@const driveSource = data.connectedSources.find(
                                (source) => source.sourceType === 'google_drive',
                            )}
                            {@const gmailSource = data.connectedSources.find(
                                (source) => source.sourceType === 'gmail',
                            )}
                            <div class="space-y-4">
                                <!-- Integration Actions -->
                                <div class="flex gap-2">
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        onclick={() => goto('/admin/integrations/google/configure')}
                                        class="cursor-pointer">
                                        Configure
                                    </Button>
                                    <AlertDialog.Root>
                                        <AlertDialog.Trigger
                                            class={buttonVariants({
                                                variant: 'destructive',
                                                size: 'sm',
                                            })}
                                            disabled={!!disconnectingSourceId}>
                                            {disconnectingSourceId
                                                ? 'Disconnecting...'
                                                : 'Disconnect'}
                                        </AlertDialog.Trigger>
                                        <AlertDialog.Content>
                                            <AlertDialog.Header>
                                                <AlertDialog.Title
                                                    >Disconnect Google Workspace?</AlertDialog.Title>
                                                <AlertDialog.Description>
                                                    This will disconnect both Google Drive and Gmail
                                                    sources and remove access credentials. Existing
                                                    indexed documents will remain searchable.
                                                </AlertDialog.Description>
                                            </AlertDialog.Header>
                                            <AlertDialog.Footer>
                                                <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
                                                <AlertDialog.Action
                                                    onclick={() => {
                                                        if (driveSource)
                                                            disconnectSource(driveSource.id)
                                                        if (gmailSource)
                                                            disconnectSource(gmailSource.id)
                                                    }}>
                                                    Disconnect
                                                </AlertDialog.Action>
                                            </AlertDialog.Footer>
                                        </AlertDialog.Content>
                                    </AlertDialog.Root>
                                </div>

                                <!-- Connected Apps -->
                                <div class="space-y-2">
                                    <div class="text-muted-foreground text-sm font-medium">
                                        Apps
                                    </div>

                                    <!-- Google Drive App -->
                                    {#if driveSource}
                                        {@const driveSync = getLatestSyncForSource(driveSource.id)}
                                        <div
                                            class="bg-muted/10 flex items-center justify-between rounded-md border p-2.5">
                                            <div class="flex items-center gap-3">
                                                <img
                                                    src={googleDriveLogo}
                                                    alt="Google Drive"
                                                    class="h-5 w-5" />
                                                <div class="flex items-center gap-2">
                                                    <span class="text-sm font-medium"
                                                        >Google Drive</span>
                                                    {#if driveSource.isActive}
                                                        <span
                                                            class="inline-flex items-center rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                                            Active
                                                        </span>
                                                    {:else}
                                                        <span
                                                            class="inline-flex items-center rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-800 dark:bg-gray-800 dark:text-gray-300">
                                                            Inactive
                                                        </span>
                                                    {/if}
                                                    {#if driveSync && driveSync.status === 'running'}
                                                        <span
                                                            class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(driveSync.status)}`}>
                                                            Syncing
                                                        </span>
                                                    {/if}
                                                </div>
                                            </div>
                                            <Button
                                                variant="ghost"
                                                size="sm"
                                                onclick={() => syncSource(driveSource.id)}
                                                disabled={syncingSourceId === driveSource.id}>
                                                {syncingSourceId === driveSource.id
                                                    ? 'Syncing...'
                                                    : 'Sync Now'}
                                            </Button>
                                        </div>
                                    {/if}

                                    <!-- Gmail App -->
                                    {#if gmailSource}
                                        {@const gmailSync = getLatestSyncForSource(gmailSource.id)}
                                        <div
                                            class="bg-muted/10 flex items-center justify-between rounded-md border p-2.5">
                                            <div class="flex items-center gap-3">
                                                <img src={gmailLogo} alt="Gmail" class="h-5 w-5" />
                                                <div class="flex items-center gap-2">
                                                    <span class="text-sm font-medium">Gmail</span>
                                                    {#if gmailSource.isActive}
                                                        <span
                                                            class="inline-flex items-center rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                                            Active
                                                        </span>
                                                    {:else}
                                                        <span
                                                            class="inline-flex items-center rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-800 dark:bg-gray-800 dark:text-gray-300">
                                                            Inactive
                                                        </span>
                                                    {/if}
                                                    {#if gmailSync && gmailSync.status === 'running'}
                                                        <span
                                                            class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(gmailSync.status)}`}>
                                                            Syncing
                                                        </span>
                                                    {/if}
                                                </div>
                                            </div>
                                            <Button
                                                variant="ghost"
                                                size="sm"
                                                onclick={() => syncSource(gmailSource.id)}
                                                disabled={syncingSourceId === gmailSource.id}>
                                                {syncingSourceId === gmailSource.id
                                                    ? 'Syncing...'
                                                    : 'Sync Now'}
                                            </Button>
                                        </div>
                                    {/if}
                                </div>
                            </div>
                        {:else}
                            {@const latestSync = getLatestSyncForSource(connectedSource.id)}
                            <div class="space-y-3">
                                <div>
                                    <div class="text-sm font-medium">Last Sync</div>
                                    <div class="text-muted-foreground text-sm">
                                        {formatDate(connectedSource.lastSyncAt)}
                                    </div>
                                </div>

                                {#if latestSync}
                                    <div>
                                        <div class="mb-2 text-sm font-medium">Latest Sync</div>
                                        <div class="space-y-1 text-xs">
                                            <div class="flex items-center gap-2">
                                                <span
                                                    class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(latestSync.status)}`}>
                                                    {latestSync.status}
                                                </span>
                                                <span class="text-muted-foreground"
                                                    >{latestSync.syncType}</span>
                                            </div>
                                            {#if latestSync.documentsProcessed > 0}
                                                <div class="text-muted-foreground">
                                                    {latestSync.documentsProcessed} documents processed
                                                    {#if latestSync.documentsUpdated > 0}
                                                        ({latestSync.documentsUpdated} updated)
                                                    {/if}
                                                </div>
                                            {/if}
                                            {#if latestSync.errorMessage}
                                                <div class="text-red-600 dark:text-red-400">
                                                    {latestSync.errorMessage}
                                                </div>
                                            {/if}
                                        </div>
                                    </div>
                                {/if}

                                <div class="flex gap-2">
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        onclick={() => syncSource(connectedSource.id)}
                                        disabled={syncingSourceId === connectedSource.id}>
                                        {syncingSourceId === connectedSource.id
                                            ? 'Syncing...'
                                            : 'Sync Now'}
                                    </Button>
                                    <AlertDialog.Root>
                                        <AlertDialog.Trigger
                                            class={buttonVariants({
                                                variant: 'outline',
                                                size: 'sm',
                                            })}
                                            disabled={disconnectingSourceId === connectedSource.id}>
                                            {disconnectingSourceId === connectedSource.id
                                                ? 'Disconnecting...'
                                                : 'Disconnect'}
                                        </AlertDialog.Trigger>
                                        <AlertDialog.Content>
                                            <AlertDialog.Header>
                                                <AlertDialog.Title
                                                    >Disconnect {integration.name}?</AlertDialog.Title>
                                                <AlertDialog.Description>
                                                    This will stop syncing data from {integration.name}
                                                    and remove access credentials. Existing indexed documents
                                                    will remain searchable.
                                                </AlertDialog.Description>
                                            </AlertDialog.Header>
                                            <AlertDialog.Footer>
                                                <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
                                                <AlertDialog.Action
                                                    onclick={() =>
                                                        disconnectSource(connectedSource.id)}>
                                                    Disconnect
                                                </AlertDialog.Action>
                                            </AlertDialog.Footer>
                                        </AlertDialog.Content>
                                    </AlertDialog.Root>
                                </div>
                            </div>
                        {/if}
                    {:else}
                        <Button onclick={() => openSetupDialog(integration)} class="cursor-pointer">
                            Connect {integration.name}
                        </Button>
                    {/if}
                </CardContent>
            </Card>
        {/each}
    </div>

    <!-- Recent Sync Runs -->
    {#if latestSyncRuns.length > 0}
        <Card>
            <CardHeader>
                <CardTitle>Recent Sync Activity</CardTitle>
                <CardDescription>Latest sync runs across all sources</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-2">
                    {#each latestSyncRuns as sync}
                        <div class="flex items-center justify-between rounded-lg border p-3">
                            <div class="flex-1">
                                <div class="mb-1 flex items-center gap-2">
                                    <span class="text-sm font-medium"
                                        >{sync.sourceName || 'Unknown Source'}</span>
                                    <span
                                        class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(sync.status)}`}>
                                        {sync.status}
                                    </span>
                                    <span class="text-muted-foreground text-xs"
                                        >{sync.syncType}</span>
                                </div>
                                <div class="text-muted-foreground text-xs">
                                    Started: {new Date(sync.startedAt).toLocaleString()}
                                    {#if sync.completedAt}
                                        • Completed: {new Date(sync.completedAt).toLocaleString()}
                                    {/if}
                                </div>
                            </div>
                            <div class="text-right">
                                <div class="text-sm font-medium">
                                    {sync.documentsProcessed || 0}
                                </div>
                                <div class="text-muted-foreground text-xs">documents</div>
                            </div>
                        </div>
                    {/each}
                </div>
            </CardContent>
        </Card>
    {/if}
</div>

<!-- Service Account Setup Dialog -->
<Dialog.Root bind:open={showSetupDialog}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect {selectedIntegration?.name}</Dialog.Title>
            <Dialog.Description>
                Set up your {selectedIntegration?.name} integration using service account credentials.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            {#if selectedIntegration?.id === 'google'}
                <div class="space-y-2">
                    <Label for="service-account-json">Service Account JSON Key</Label>
                    <Textarea
                        id="service-account-json"
                        bind:value={serviceAccountJson}
                        placeholder="Paste your Google service account JSON key here..."
                        rows={10}
                        class="max-h-64 overflow-y-auto font-mono text-sm break-all whitespace-pre-wrap" />
                    <p class="text-muted-foreground text-sm">
                        Download this from the Google Cloud Console under "Service Accounts" >
                        "Keys".
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="principal-email">Admin Email</Label>
                    <Input
                        id="principal-email"
                        bind:value={principalEmail}
                        placeholder="admin@yourdomain.com"
                        type="email"
                        required />
                    <p class="text-muted-foreground text-sm">
                        The admin user email that the service account will impersonate to access
                        Google Workspace APIs.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="domain">Organization Domain</Label>
                    <Input
                        id="domain"
                        bind:value={domain}
                        placeholder="yourdomain.com"
                        type="text"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Your Google Workspace domain (e.g., company.com). The service account will
                        impersonate all users in this domain.
                    </p>
                </div>
            {:else if selectedIntegration?.id === 'atlassian'}
                <div class="space-y-2">
                    <Label for="principal-email">Your Atlassian Email</Label>
                    <Input
                        id="principal-email"
                        bind:value={principalEmail}
                        placeholder="your.email@company.com"
                        type="email"
                        required />
                </div>

                <div class="space-y-2">
                    <Label for="api-token">API Token</Label>
                    <Input
                        id="api-token"
                        bind:value={apiToken}
                        placeholder="Your Atlassian API token"
                        type="password"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Create an API token at <a
                            href="https://id.atlassian.com/manage-profile/security/api-tokens"
                            target="_blank"
                            class="text-blue-600 hover:underline">id.atlassian.com</a>
                    </p>
                </div>
            {:else if selectedIntegration?.id === 'slack'}
                <div class="space-y-2">
                    <Label for="bot-token">Bot Token</Label>
                    <Input
                        id="bot-token"
                        bind:value={apiToken}
                        placeholder="xoxb-your-slack-bot-token"
                        type="password"
                        required />
                    <p class="text-muted-foreground text-sm">
                        Get this from your Slack app settings under "OAuth & Permissions".
                    </p>
                </div>
            {/if}
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={() => (showSetupDialog = false)}>Cancel</Button>
            <Button onclick={setupServiceAccount} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
