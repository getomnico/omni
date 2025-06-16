<script lang="ts">
    import type { PageData } from './$types'
    import { page } from '$app/stores'

    export let data: PageData

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }

    function getSourceByType(providerId: string) {
        // Map OAuth provider IDs to their associated source types
        if (providerId === 'google') {
            return data.connectedSources.find((source) => 
                source.sourceType === 'google_drive' || source.sourceType === 'gmail'
            )
        } else if (providerId === 'atlassian') {
            return data.connectedSources.find((source) => 
                source.sourceType === 'confluence' || source.sourceType === 'jira'
            )
        }
        // For other providers, the ID matches the source type
        return data.connectedSources.find((source) => source.sourceType === providerId)
    }

    // Handle success/error messages from OAuth flow
    let message = ''
    let messageType: 'success' | 'error' | '' = ''

    if ($page.url.searchParams.get('success') === 'google_connected') {
        message = 'Google Workspace has been successfully connected!'
        messageType = 'success'
    } else if ($page.url.searchParams.get('error') === 'oauth_denied') {
        message = 'OAuth connection was denied or failed.'
        messageType = 'error'
    }
</script>

<div class="bg-background min-h-screen">
    <nav class="bg-card border-b shadow">
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
            <div class="flex h-16 justify-between">
                <div class="flex items-center">
                    <h1 class="text-xl font-semibold">Settings - Integrations</h1>
                </div>
                <div class="flex items-center space-x-4">
                    <a href="/" class="text-muted-foreground hover:text-foreground text-sm"
                        >Back to Home</a
                    >
                </div>
            </div>
        </div>
    </nav>

    <main class="mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8">
        {#if message}
            <div
                class="mb-6 rounded-md p-4 {messageType === 'success'
                    ? 'border border-green-200 bg-green-50 text-green-800 dark:border-green-800 dark:bg-green-900/20 dark:text-green-400'
                    : 'border border-red-200 bg-red-50 text-red-800 dark:border-red-800 dark:bg-red-900/20 dark:text-red-400'}"
            >
                <p class="text-sm font-medium">{message}</p>
            </div>
        {/if}

        <div class="bg-card rounded-lg border shadow">
            <div class="px-4 py-5 sm:p-6">
                <h2 class="text-foreground mb-2 text-lg font-medium">Connected Data Sources</h2>
                <p class="text-muted-foreground mb-6 text-sm">
                    View the data sources that have been connected to your organization. Only
                    administrators can connect or disconnect integrations.
                </p>

                <div class="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3">
                    {#each data.availableIntegrations as integration (integration.id)}
                        {@const connectedSource = getSourceByType(integration.id)}
                        <div class="bg-background rounded-lg border p-6">
                            <div class="mb-4 flex items-center justify-between">
                                <div class="flex items-center space-x-3">
                                    <span class="text-2xl">{integration.icon}</span>
                                    <h3 class="text-foreground text-lg font-medium">
                                        {integration.name}
                                    </h3>
                                </div>
                                <div class="flex items-center">
                                    {#if integration.connected}
                                        <span
                                            class="inline-flex items-center rounded-full bg-green-100 px-2.5 py-0.5 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400"
                                        >
                                            <span class="mr-1 h-1.5 w-1.5 rounded-full bg-green-400"
                                            ></span>
                                            Connected
                                        </span>
                                    {:else}
                                        <span
                                            class="inline-flex items-center rounded-full bg-gray-100 px-2.5 py-0.5 text-xs font-medium text-gray-800 dark:bg-gray-800 dark:text-gray-300"
                                        >
                                            <span class="mr-1 h-1.5 w-1.5 rounded-full bg-gray-400"
                                            ></span>
                                            Not Connected
                                        </span>
                                    {/if}
                                </div>
                            </div>

                            <p class="text-muted-foreground mb-4 text-sm">
                                {integration.description}
                            </p>

                            {#if integration.connected && connectedSource}
                                <div class="space-y-2 text-sm">
                                    <div class="flex justify-between">
                                        <span class="text-muted-foreground">Last Sync:</span>
                                        <span class="text-foreground"
                                            >{formatDate(connectedSource.lastSyncAt)}</span
                                        >
                                    </div>
                                    <div class="flex justify-between">
                                        <span class="text-muted-foreground">Status:</span>
                                        <span
                                            class={connectedSource.isActive
                                                ? 'text-green-600 dark:text-green-400'
                                                : 'text-destructive'}
                                        >
                                            {connectedSource.isActive ? 'Active' : 'Inactive'}
                                        </span>
                                    </div>
                                    <div class="flex justify-between">
                                        <span class="text-muted-foreground">Connected:</span>
                                        <span class="text-foreground"
                                            >{formatDate(connectedSource.createdAt)}</span
                                        >
                                    </div>
                                </div>
                            {:else}
                                <div class="text-center">
                                    <p class="text-muted-foreground mb-3 text-sm">
                                        Not connected yet
                                    </p>
                                    <p class="text-muted-foreground text-xs">
                                        Contact your administrator to connect this integration
                                    </p>
                                </div>
                            {/if}
                        </div>
                    {/each}
                </div>

                {#if data.connectedSources.length > 0}
                    <div class="mt-8">
                        <h3 class="text-foreground mb-4 text-lg font-medium">
                            Available Data Sources
                        </h3>
                        <p class="text-muted-foreground mb-4 text-sm">
                            You can search across all of these connected data sources from the main
                            search page.
                        </p>
                        <div class="ring-border overflow-hidden shadow ring-1 md:rounded-lg">
                            <table class="divide-border min-w-full divide-y">
                                <thead class="bg-muted/50">
                                    <tr>
                                        <th
                                            class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                            >Source</th
                                        >
                                        <th
                                            class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                            >Type</th
                                        >
                                        <th
                                            class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                            >Status</th
                                        >
                                        <th
                                            class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                            >Last Sync</th
                                        >
                                    </tr>
                                </thead>
                                <tbody class="divide-border bg-background divide-y">
                                    {#each data.connectedSources as source (source.id)}
                                        <tr>
                                            <td
                                                class="text-foreground px-6 py-4 text-sm font-medium"
                                                >{source.name}</td
                                            >
                                            <td
                                                class="text-muted-foreground px-6 py-4 text-sm capitalize"
                                                >{source.sourceType}</td
                                            >
                                            <td class="px-6 py-4 text-sm">
                                                <span
                                                    class="inline-flex rounded-full px-2 text-xs leading-5 font-semibold
													{source.isActive
                                                        ? 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
                                                        : 'bg-destructive/10 text-destructive'}
												"
                                                >
                                                    {source.isActive ? 'Active' : 'Inactive'}
                                                </span>
                                            </td>
                                            <td class="text-muted-foreground px-6 py-4 text-sm"
                                                >{formatDate(source.lastSyncAt)}</td
                                            >
                                        </tr>
                                    {/each}
                                </tbody>
                            </table>
                        </div>
                    </div>
                {:else}
                    <div class="mt-8 py-12 text-center">
                        <h3 class="text-foreground mb-2 text-lg font-medium">
                            No Integrations Connected
                        </h3>
                        <p class="text-muted-foreground mb-4 text-sm">
                            Your organization hasn't connected any data sources yet.
                        </p>
                        <p class="text-muted-foreground text-xs">
                            Contact your administrator to set up integrations with Google, Slack,
                            and other services.
                        </p>
                    </div>
                {/if}
            </div>
        </div>
    </main>
</div>
