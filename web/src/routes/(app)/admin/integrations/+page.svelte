<script lang="ts">
    import type { PageData } from './$types'

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
</script>

<div class="bg-background min-h-screen">
    <nav class="bg-card border-b shadow">
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
            <div class="flex h-16 justify-between">
                <div class="flex items-center">
                    <h1 class="text-xl font-semibold">Clio Admin - Integrations</h1>
                </div>
                <div class="flex items-center space-x-4">
                    <a
                        href="/admin/users"
                        class="text-muted-foreground hover:text-foreground text-sm"
                        >User Management</a
                    >
                    <a href="/" class="text-muted-foreground hover:text-foreground text-sm"
                        >Back to Home</a
                    >
                </div>
            </div>
        </div>
    </nav>

    <main class="mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8">
        <div class="bg-card rounded-lg border shadow">
            <div class="px-4 py-5 sm:p-6">
                <h2 class="text-foreground mb-2 text-lg font-medium">Data Source Integrations</h2>
                <p class="text-muted-foreground mb-6 text-sm">
                    Connect organization-wide data sources. Once connected, all users can search
                    across these data sources.
                </p>

                <div class="grid grid-cols-1 gap-6 sm:grid-cols-2 lg:grid-cols-3">
                    {#each data.availableIntegrations as integration (integration.id)}
                        {@const connectedSource = getSourceByType(integration.id)}
                        <div
                            class="bg-background rounded-lg border p-6 transition-shadow hover:shadow-md"
                        >
                            <div class="mb-4 flex items-center justify-between">
                                <div class="flex items-center space-x-3">
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
                                <div class="mb-4 space-y-2 text-sm">
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
                                        <span class="text-muted-foreground">Sync Status:</span>
                                        <span class="text-foreground"
                                            >{connectedSource.syncStatus || 'Unknown'}</span
                                        >
                                    </div>
                                </div>

                                <div class="flex space-x-2">
                                    <button
                                        class="bg-muted hover:bg-muted/80 text-foreground flex-1 rounded-md px-4 py-2 text-sm font-medium transition-colors"
                                    >
                                        Sync Now
                                    </button>
                                    <button
                                        class="border-destructive text-destructive hover:bg-destructive hover:text-destructive-foreground rounded-md border px-4 py-2 text-sm font-medium transition-colors"
                                    >
                                        Disconnect
                                    </button>
                                </div>
                            {:else}
                                {#if integration.id === 'google'}
                                    <!-- Official Google Sign-in Button -->
                                    <a
                                        href={integration.connectUrl}
                                        class="inline-flex w-full items-center justify-center rounded-md border border-gray-300 bg-white px-4 py-2 text-sm font-medium text-gray-700 shadow-sm transition-colors hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2"
                                    >
                                        <!-- Google G Logo SVG -->
                                        <svg class="mr-3 h-5 w-5" viewBox="0 0 24 24">
                                            <path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
                                            <path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/>
                                            <path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/>
                                            <path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/>
                                        </svg>
                                        Connect with Google
                                    </a>
                                {:else if integration.id === 'slack'}
                                    <!-- Official Slack Add to Slack Button -->
                                    <a
                                        href={integration.connectUrl}
                                        class="inline-flex w-full items-center justify-center"
                                    >
                                        <img
                                            alt="Add to Slack"
                                            height="40"
                                            width="139"
                                            src="https://platform.slack-edge.com/img/add_to_slack.png"
                                            srcset="https://platform.slack-edge.com/img/add_to_slack.png 1x, https://platform.slack-edge.com/img/add_to_slack@2x.png 2x"
                                        />
                                    </a>
                                {:else if integration.id === 'atlassian'}
                                    <!-- Atlassian Connect Button -->
                                    <a
                                        href={integration.connectUrl}
                                        class="inline-flex w-full items-center justify-center rounded-md bg-[#0052CC] px-4 py-2 text-sm font-medium text-white transition-colors hover:bg-[#0747A6] focus:outline-none focus:ring-2 focus:ring-[#0052CC] focus:ring-offset-2"
                                    >
                                        <!-- Atlassian Logo -->
                                        <svg class="mr-2 h-5 w-5" viewBox="0 0 24 24" fill="none">
                                            <path d="M7.99 11.411c-.267-.415-.81-.299-.915.195l-2.757 12.96c-.066.31.162.599.476.599h5.34c.196 0 .37-.127.43-.313l2.538-7.853c.209-.647-.745-1.115-1.011-.496L7.99 11.411zM11.348.305a.477.477 0 00-.9.007L4.543 15.001a.477.477 0 00.432.687h5.452c.196 0 .37-.126.43-.312l3.04-9.404c.206-.637-.735-1.105-1.01-.496L11.348.305z" fill="currentColor"/>
                                        </svg>
                                        Connect with Atlassian
                                    </a>
                                {:else}
                                    <a
                                        href={integration.connectUrl}
                                        class="text-primary-foreground bg-primary hover:bg-primary/90 inline-flex w-full items-center justify-center rounded-md border border-transparent px-4 py-2 text-sm font-medium transition-colors"
                                    >
                                        Connect to {integration.name}
                                    </a>
                                {/if}
                            {/if}
                        </div>
                    {/each}
                </div>

                {#if data.connectedSources.length > 0}
                    <div class="mt-8">
                        <h3 class="text-foreground mb-4 text-lg font-medium">
                            Connected Sources Details
                        </h3>
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
                                        <th
                                            class="text-foreground px-6 py-3 text-left text-sm font-semibold"
                                            >Created</th
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
                                            <td class="text-muted-foreground px-6 py-4 text-sm"
                                                >{formatDate(source.createdAt)}</td
                                            >
                                        </tr>
                                    {/each}
                                </tbody>
                            </table>
                        </div>
                    </div>
                {/if}
            </div>
        </div>
    </main>
</div>
