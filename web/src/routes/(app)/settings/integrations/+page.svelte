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
            return data.connectedSources.find(
                (source) => source.sourceType === 'google_drive' || source.sourceType === 'gmail',
            )
        } else if (providerId === 'atlassian') {
            return data.connectedSources.find(
                (source) => source.sourceType === 'confluence' || source.sourceType === 'jira',
            )
        }
        // For other providers, the ID matches the source type
        return data.connectedSources.find((source) => source.sourceType === providerId)
    }

    // Handle success/error messages from OAuth flow
    let message = ''
    let messageType: 'success' | 'error' | '' = ''

    if (data.message) {
        message = data.message.text
        messageType = data.message.type
    } else if ($page.url.searchParams.get('success') === 'google_connected') {
        message = 'Google Workspace has been successfully connected!'
        messageType = 'success'
    } else if ($page.url.searchParams.get('error') === 'oauth_denied') {
        message = 'OAuth connection was denied or failed.'
        messageType = 'error'
    }

    function formatProfileName(profileData: any): string {
        if (profileData.name) return profileData.name
        if (profileData.given_name && profileData.family_name) {
            return `${profileData.given_name} ${profileData.family_name}`
        }
        return profileData.email || 'Unknown'
    }

    function unlinkOAuthAccount(provider: string, providerUserId: string) {
        const form = document.createElement('form')
        form.method = 'POST'
        form.action = `/auth/${provider}/unlink`

        const input = document.createElement('input')
        input.type = 'hidden'
        input.name = 'provider_user_id'
        input.value = providerUserId

        form.appendChild(input)
        document.body.appendChild(form)
        form.submit()
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

        <!-- Personal OAuth Accounts Section -->
        <div class="bg-card mb-8 rounded-lg border shadow">
            <div class="px-4 py-5 sm:p-6">
                <h2 class="text-foreground mb-2 text-lg font-medium">Personal Accounts</h2>
                <p class="text-muted-foreground mb-6 text-sm">
                    Manage your personal OAuth accounts linked to your Clio account. These accounts
                    are used for authentication and may improve search results based on your access
                    permissions.
                </p>

                {#if data.userOAuthCredentials.length > 0}
                    <div class="space-y-4">
                        {#each data.userOAuthCredentials as credential (credential.id)}
                            <div class="bg-background rounded-lg border p-4">
                                <div class="flex items-center justify-between">
                                    <div class="flex items-center space-x-3">
                                        <div
                                            class="flex h-10 w-10 items-center justify-center rounded-full bg-blue-100 dark:bg-blue-900/20"
                                        >
                                            {#if credential.provider === 'google'}
                                                <svg class="h-5 w-5" viewBox="0 0 24 24">
                                                    <path
                                                        d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
                                                        fill="#4285F4"
                                                    />
                                                    <path
                                                        d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
                                                        fill="#34A853"
                                                    />
                                                    <path
                                                        d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
                                                        fill="#FBBC05"
                                                    />
                                                    <path
                                                        d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
                                                        fill="#EA4335"
                                                    />
                                                </svg>
                                            {:else}
                                                <span class="text-lg">🔗</span>
                                            {/if}
                                        </div>
                                        <div>
                                            <h3 class="text-foreground text-sm font-medium">
                                                {formatProfileName(credential.profile_data)}
                                            </h3>
                                            <p class="text-muted-foreground text-xs">
                                                {credential.profile_data.email}
                                                {#if credential.profile_data.hd}
                                                    • {credential.profile_data.hd}
                                                {/if}
                                            </p>
                                        </div>
                                    </div>
                                    <div class="flex items-center space-x-2">
                                        <span class="text-muted-foreground text-xs capitalize"
                                            >{credential.provider}</span
                                        >
                                        <button
                                            class="text-destructive hover:text-destructive/80 text-xs font-medium"
                                            onclick={() =>
                                                unlinkOAuthAccount(
                                                    credential.provider,
                                                    credential.provider_user_id,
                                                )}
                                        >
                                            Unlink
                                        </button>
                                    </div>
                                </div>
                                <div class="mt-3 flex items-center space-x-4 text-xs">
                                    <span class="text-muted-foreground">
                                        Connected: {formatDate(credential.created_at)}
                                    </span>
                                    <span class="text-muted-foreground">
                                        Last Updated: {formatDate(credential.updated_at)}
                                    </span>
                                </div>
                            </div>
                        {/each}
                    </div>
                {:else}
                    <div class="py-6 text-center">
                        <p class="text-muted-foreground mb-4 text-sm">
                            No personal accounts linked yet.
                        </p>
                        <div class="space-y-2">
                            <a
                                href="/auth/google/link"
                                class="inline-flex items-center rounded-md bg-blue-600 px-3 py-2 text-sm font-medium text-white hover:bg-blue-700"
                            >
                                <svg class="mr-2 h-4 w-4" viewBox="0 0 24 24">
                                    <path
                                        d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
                                        fill="#4285F4"
                                    />
                                    <path
                                        d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
                                        fill="#34A853"
                                    />
                                    <path
                                        d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"
                                        fill="#FBBC05"
                                    />
                                    <path
                                        d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
                                        fill="#EA4335"
                                    />
                                </svg>
                                Link Google Account
                            </a>
                        </div>
                    </div>
                {/if}
            </div>
        </div>

        <!-- Organization Data Sources Section -->
        <div class="bg-card rounded-lg border shadow">
            <div class="px-4 py-5 sm:p-6">
                <h2 class="text-foreground mb-2 text-lg font-medium">Organization Data Sources</h2>
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
