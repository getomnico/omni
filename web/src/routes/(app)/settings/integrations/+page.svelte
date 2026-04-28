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
    import { Globe, HardDrive, Mail } from '@lucide/svelte'
    import GoogleOAuthSetup from '$lib/components/google-oauth-setup.svelte'
    import { getSourceIconPath } from '$lib/utils/icons'
    import { enhance } from '$app/forms'

    let { data }: PageProps = $props()

    let showGoogleOAuthSetup = $state(false)

    let hasGoogleDrive = $derived(data.userSources.some((s) => s.sourceType === 'google_drive'))
    let hasGmail = $derived(data.userSources.some((s) => s.sourceType === 'gmail'))
    let hasAllGoogleSources = $derived(hasGoogleDrive && hasGmail)

    function handleGoogleOAuthSetupSuccess() {
        showGoogleOAuthSetup = false
        window.location.reload()
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
            <p class="text-muted-foreground mt-2">Apps that are currently connected with Omni</p>
        </div>

        <!-- Org-wide Sources -->
        {#if data.orgWideSources.length > 0}
            <div class="space-y-4">
                <h2 class="text-xl font-semibold">Organization</h2>
                <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                    {#each data.orgWideSources as source}
                        <div class="flex items-center gap-3 rounded-lg border p-4">
                            {#if getSourceIconPath(source.sourceType)}
                                <img
                                    src={getSourceIconPath(source.sourceType)}
                                    alt={source.name}
                                    class="h-6 w-6" />
                            {:else if source.sourceType === 'web'}
                                <Globe class="text-muted-foreground h-6 w-6" />
                            {:else if source.sourceType === 'local_files'}
                                <HardDrive class="text-muted-foreground h-6 w-6" />
                            {:else if source.sourceType === 'imap'}
                                <Mail class="text-muted-foreground h-6 w-6" />
                            {/if}
                            <span class="truncate font-medium">{source.name}</span>
                        </div>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- User's Own Sources -->
        {#if data.userSources.length > 0}
            <div class="space-y-4">
                <h2 class="text-xl font-semibold">Your Connections</h2>
                <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                    {#each data.userSources as source}
                        <div class="flex flex-col gap-3 rounded-lg border p-4">
                            <div class="flex items-center gap-3">
                                {#if getSourceIconPath(source.sourceType)}
                                    <img
                                        src={getSourceIconPath(source.sourceType)}
                                        alt={source.name}
                                        class="h-6 w-6" />
                                {:else if source.sourceType === 'web'}
                                    <Globe class="text-muted-foreground h-6 w-6" />
                                {:else if source.sourceType === 'local_files'}
                                    <HardDrive class="text-muted-foreground h-6 w-6" />
                                {:else if source.sourceType === 'imap'}
                                    <Mail class="text-muted-foreground h-6 w-6" />
                                {/if}
                                <span class="truncate font-medium">{source.name}</span>
                                <span
                                    class={`ml-auto inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ${getStatusColor(source.isActive)}`}>
                                    {source.isActive ? 'Enabled' : 'Disabled'}
                                </span>
                            </div>
                            <form
                                method="POST"
                                action="?/{source.isActive ? 'disable' : 'enable'}"
                                use:enhance>
                                <input type="hidden" name="sourceId" value={source.id} />
                                <Button
                                    type="submit"
                                    variant={source.isActive ? 'outline' : 'default'}
                                    size="sm"
                                    class="w-full cursor-pointer">
                                    {source.isActive ? 'Disable' : 'Enable'}
                                </Button>
                            </form>
                        </div>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- Available Connections -->
        {#if data.googleOAuthConfigured}
            <div class="space-y-4">
                <div>
                    <h2 class="text-xl font-semibold">Available Connections</h2>
                    <p class="text-muted-foreground text-sm">
                        Connect your own accounts to sync data with Omni
                    </p>
                </div>

                <div class="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
                    <Card class="flex flex-col">
                        <CardHeader>
                            <CardTitle class="flex items-center gap-3">
                                <img src={googleLogo} alt="Google" class="h-6 w-6" />
                                <span>Google</span>
                            </CardTitle>
                            <CardDescription>
                                Connect your Google Drive and Gmail with read-only access. Your data
                                stays private to you.
                            </CardDescription>
                        </CardHeader>
                        <CardContent class="flex-1" />
                        <CardFooter>
                            {#if hasAllGoogleSources}
                                <span class="text-sm font-medium text-green-500"> Connected </span>
                            {:else}
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => (showGoogleOAuthSetup = true)}>
                                    Connect with Google
                                </Button>
                            {/if}
                        </CardFooter>
                    </Card>
                </div>
            </div>
        {:else if data.orgWideSources.length === 0 && data.userSources.length === 0}
            <div class="py-12 text-center">
                <p class="text-muted-foreground text-sm">
                    No integrations are available yet. Contact your administrator to set up
                    connections.
                </p>
            </div>
        {/if}
    </div>
</div>

<GoogleOAuthSetup
    open={showGoogleOAuthSetup}
    connectedSourceTypes={data.userSources.map((s) => s.sourceType)}
    onSuccess={handleGoogleOAuthSetupSuccess}
    onCancel={() => (showGoogleOAuthSetup = false)} />
