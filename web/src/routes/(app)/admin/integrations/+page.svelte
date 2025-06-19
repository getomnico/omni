<script lang="ts">
    import type { PageData } from './$types'
    import { onMount, onDestroy } from 'svelte'
    import { Button } from '$lib/components/ui/button'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Input } from '$lib/components/ui/input'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import * as Dialog from '$lib/components/ui/dialog'
    import { toast } from 'svelte-sonner'
    import { ServiceProvider, AuthType } from '$lib/types'

    export let data: PageData

    let liveIndexingStatus = data.indexingStatus
    let eventSource: EventSource | null = null
    let disconnectingSourceId: string | null = null
    let syncingSourceId: string | null = null

    // Service account setup state
    let showSetupDialog = false
    let selectedIntegration: any = null
    let serviceAccountJson = ''
    let apiToken = ''
    let principalEmail = ''
    let delegatedUser = ''
    let isSubmitting = false

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }

    function getSourceByType(providerId: string) {
        if (providerId === 'google') {
            return data.connectedSources.find(
                (source) => source.sourceType === 'google_drive' || source.sourceType === 'gmail',
            )
        } else if (providerId === 'atlassian') {
            return data.connectedSources.find(
                (source) => source.sourceType === 'confluence' || source.sourceType === 'jira',
            )
        }
        return data.connectedSources.find((source) => source.sourceType === providerId)
    }

    function getIndexingStatus(sourceId: string) {
        const status = liveIndexingStatus[sourceId] || {}
        return {
            pending: status.pending || 0,
            processing: status.processing || 0,
            completed: status.completed || 0,
            failed: status.failed || 0,
            total:
                (status.pending || 0) +
                (status.processing || 0) +
                (status.completed || 0) +
                (status.failed || 0),
        }
    }

    function getStatusColor(status: string) {
        switch (status) {
            case 'processing':
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

    function getOverallStats() {
        let overall = { pending: 0, processing: 0, completed: 0, failed: 0, total: 0 }
        Object.values(liveIndexingStatus).forEach((sourceStatus: any) => {
            overall.pending += sourceStatus.pending || 0
            overall.processing += sourceStatus.processing || 0
            overall.completed += sourceStatus.completed || 0
            overall.failed += sourceStatus.failed || 0
        })
        overall.total = overall.pending + overall.processing + overall.completed + overall.failed
        return overall
    }

    function openSetupDialog(integration: any) {
        selectedIntegration = integration
        showSetupDialog = true
        serviceAccountJson = ''
        apiToken = ''
        principalEmail = ''
        delegatedUser = ''
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

                // Validate JSON
                try {
                    JSON.parse(serviceAccountJson)
                } catch {
                    throw new Error('Invalid JSON format')
                }

                credentials = { service_account_key: serviceAccountJson }
                config = {
                    scopes: ['https://www.googleapis.com/auth/drive.readonly'],
                    delegated_user: delegatedUser || null,
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

            // First create the source
            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: `${selectedIntegration.name} Source`,
                    sourceType:
                        selectedIntegration.id === 'google'
                            ? 'google_drive'
                            : selectedIntegration.id,
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create source')
            }

            const { source } = await sourceResponse.json()

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

            toast.success(`${selectedIntegration.name} connected successfully!`)
            showSetupDialog = false
            window.location.reload()
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
        eventSource = new EventSource('/api/indexing-status/stream')
        eventSource.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data)
                liveIndexingStatus = data
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

    $: overallStats = getOverallStats()
</script>

<svelte:head>
    <title>Integrations - Clio Admin</title>
</svelte:head>

<div class="space-y-6">
    <div>
        <h1 class="text-2xl font-bold tracking-tight">Integrations</h1>
        <p class="text-muted-foreground">
            Connect Clio to your data sources using service accounts and API tokens.
        </p>
    </div>

    <!-- Overall Indexing Status -->
    {#if overallStats.total > 0}
        <Card>
            <CardHeader>
                <CardTitle>Overall Indexing Status</CardTitle>
                <CardDescription
                    >Real-time status of document processing across all sources</CardDescription
                >
            </CardHeader>
            <CardContent>
                <div class="grid grid-cols-2 gap-4 md:grid-cols-5">
                    <div class="text-center">
                        <div class="text-2xl font-bold text-yellow-600">{overallStats.pending}</div>
                        <div class="text-muted-foreground text-sm">Pending</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-blue-600">
                            {overallStats.processing}
                        </div>
                        <div class="text-muted-foreground text-sm">Processing</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-green-600">
                            {overallStats.completed}
                        </div>
                        <div class="text-muted-foreground text-sm">Completed</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-red-600">{overallStats.failed}</div>
                        <div class="text-muted-foreground text-sm">Failed</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold">{overallStats.total}</div>
                        <div class="text-muted-foreground text-sm">Total</div>
                    </div>
                </div>
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
                        {integration.name}
                        {#if connectedSource}
                            <span
                                class="inline-flex items-center rounded-full bg-green-100 px-2 py-1 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400"
                            >
                                Connected
                            </span>
                        {:else}
                            <span
                                class="inline-flex items-center rounded-full bg-gray-100 px-2 py-1 text-xs font-medium text-gray-800 dark:bg-gray-800 dark:text-gray-300"
                            >
                                Not Connected
                            </span>
                        {/if}
                    </CardTitle>
                    <CardDescription>{integration.description}</CardDescription>
                </CardHeader>
                <CardContent>
                    {#if connectedSource}
                        {@const indexingStatus = getIndexingStatus(connectedSource.id)}
                        <div class="space-y-3">
                            <div>
                                <div class="text-sm font-medium">Last Sync</div>
                                <div class="text-muted-foreground text-sm">
                                    {formatDate(connectedSource.lastSyncAt)}
                                </div>
                            </div>

                            {#if indexingStatus.total > 0}
                                <div>
                                    <div class="mb-2 text-sm font-medium">Indexing Status</div>
                                    <div class="grid grid-cols-2 gap-2 text-xs">
                                        <div class="flex justify-between">
                                            <span>Pending:</span>
                                            <span class="font-medium">{indexingStatus.pending}</span
                                            >
                                        </div>
                                        <div class="flex justify-between">
                                            <span>Processing:</span>
                                            <span class="font-medium"
                                                >{indexingStatus.processing}</span
                                            >
                                        </div>
                                        <div class="flex justify-between">
                                            <span>Completed:</span>
                                            <span class="font-medium"
                                                >{indexingStatus.completed}</span
                                            >
                                        </div>
                                        <div class="flex justify-between">
                                            <span>Failed:</span>
                                            <span class="font-medium">{indexingStatus.failed}</span>
                                        </div>
                                    </div>
                                </div>
                            {/if}

                            <div class="flex gap-2">
                                <Button
                                    variant="outline"
                                    size="sm"
                                    on:click={() => syncSource(connectedSource.id)}
                                    disabled={syncingSourceId === connectedSource.id}
                                >
                                    {syncingSourceId === connectedSource.id
                                        ? 'Syncing...'
                                        : 'Sync Now'}
                                </Button>
                                <AlertDialog.Root>
                                    <AlertDialog.Trigger asChild let:builder>
                                        <Button
                                            builders={[builder]}
                                            variant="outline"
                                            size="sm"
                                            disabled={disconnectingSourceId === connectedSource.id}
                                        >
                                            {disconnectingSourceId === connectedSource.id
                                                ? 'Disconnecting...'
                                                : 'Disconnect'}
                                        </Button>
                                    </AlertDialog.Trigger>
                                    <AlertDialog.Content>
                                        <AlertDialog.Header>
                                            <AlertDialog.Title
                                                >Disconnect {integration.name}?</AlertDialog.Title
                                            >
                                            <AlertDialog.Description>
                                                This will stop syncing data from {integration.name} and
                                                remove access credentials. Existing indexed documents
                                                will remain searchable.
                                            </AlertDialog.Description>
                                        </AlertDialog.Header>
                                        <AlertDialog.Footer>
                                            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
                                            <AlertDialog.Action
                                                on:click={() =>
                                                    disconnectSource(connectedSource.id)}
                                            >
                                                Disconnect
                                            </AlertDialog.Action>
                                        </AlertDialog.Footer>
                                    </AlertDialog.Content>
                                </AlertDialog.Root>
                            </div>
                        </div>
                    {:else}
                        <Button on:click={() => openSetupDialog(integration)}>
                            Connect {integration.name}
                        </Button>
                    {/if}
                </CardContent>
            </Card>
        {/each}
    </div>
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
                        class="font-mono text-sm"
                    />
                    <p class="text-muted-foreground text-sm">
                        Download this from the Google Cloud Console under "Service Accounts" >
                        "Keys".
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="delegated-user">Delegated User Email (Optional)</Label>
                    <Input
                        id="delegated-user"
                        bind:value={delegatedUser}
                        placeholder="user@yourdomain.com"
                        type="email"
                    />
                    <p class="text-muted-foreground text-sm">
                        If using domain-wide delegation, specify the user to impersonate.
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
                        required
                    />
                </div>

                <div class="space-y-2">
                    <Label for="api-token">API Token</Label>
                    <Input
                        id="api-token"
                        bind:value={apiToken}
                        placeholder="Your Atlassian API token"
                        type="password"
                        required
                    />
                    <p class="text-muted-foreground text-sm">
                        Create an API token at <a
                            href="https://id.atlassian.com/manage-profile/security/api-tokens"
                            target="_blank"
                            class="text-blue-600 hover:underline">id.atlassian.com</a
                        >
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
                        required
                    />
                    <p class="text-muted-foreground text-sm">
                        Get this from your Slack app settings under "OAuth & Permissions".
                    </p>
                </div>
            {/if}
        </div>

        <Dialog.Footer>
            <Button variant="outline" on:click={() => (showSetupDialog = false)}>Cancel</Button>
            <Button on:click={setupServiceAccount} disabled={isSubmitting}>
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
