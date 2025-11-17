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
    import { Globe, HardDrive } from '@lucide/svelte'
    import GoogleWorkspaceSetup from '$lib/components/google-workspace-setup.svelte'
    import AtlassianConnectorSetup from '$lib/components/atlassian-connector-setup.svelte'
    import WebConnectorSetupDialog from '$lib/components/web-connector-setup-dialog.svelte'

    let { data }: PageProps = $props()

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

    function getSourceIcon(sourceType: string) {
        switch (sourceType) {
            case 'google_drive':
                return googleDriveLogo
            case 'gmail':
                return gmailLogo
            case 'slack':
                return slackLogo
            case 'confluence':
                return confluenceLogo
            case 'jira':
                return jiraLogo
            case 'web':
                return null
            case 'local_files':
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
                            class="flex max-w-lg items-center justify-between rounded-lg border px-4 py-3">
                            <div class="flex items-center gap-3">
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
                                        <span class="font-medium">{source.name}</span>
                                        <span
                                            class={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(source.isActive)}`}>
                                            {source.isActive ? 'Active' : 'Inactive'}
                                        </span>
                                    </div>
                                    <div
                                        class="text-muted-foreground flex items-center gap-2 text-xs">
                                        <span class="capitalize"
                                            >{source.sourceType.replace('_', ' ')}</span>
                                        <span>•</span>
                                        <span>Last sync: {formatDate(source.lastSyncAt)}</span>
                                    </div>
                                </div>
                            </div>
                            <div class="flex gap-2">
                                <Button variant="outline" size="sm" class="cursor-pointer">
                                    Sync
                                </Button>
                                <Button variant="outline" size="sm" class="cursor-pointer">
                                    Configure
                                </Button>
                            </div>
                        </div>
                    {/each}
                </div>
            {:else}
                <div class="py-12 text-center">
                    <p class="text-muted-foreground">
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
