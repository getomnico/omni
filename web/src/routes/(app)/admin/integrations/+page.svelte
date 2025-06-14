<script lang="ts">
    import type { PageData } from './$types'

    export let data: PageData

    function formatDate(date: Date | null) {
        if (!date) return 'N/A'
        return new Date(date).toLocaleDateString()
    }

    function getSourceByType(sourceType: string) {
        return data.connectedSources.find((source) => source.sourceType === sourceType)
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
                                <a
                                    href={integration.connectUrl}
                                    class="text-primary-foreground bg-primary hover:bg-primary/90 inline-flex w-full items-center justify-center rounded-md border border-transparent px-4 py-2 text-sm font-medium transition-colors"
                                >
                                    Connect to {integration.name}
                                </a>
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
