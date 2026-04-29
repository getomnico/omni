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
    import { Badge } from '$lib/components/ui/badge'
    import { Switch } from '$lib/components/ui/switch'
    import * as AlertDialog from '$lib/components/ui/alert-dialog'
    import type { PageProps } from './$types'
    import googleLogo from '$lib/images/icons/google.svg'
    import { Globe, HardDrive, Mail, Trash2 } from '@lucide/svelte'
    import GoogleOAuthSetup from '$lib/components/google-oauth-setup.svelte'
    import { getSourceIconPath } from '$lib/utils/icons'
    import { invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'

    let { data }: PageProps = $props()

    type UserSource = (typeof data.userSources)[number]

    let sourceToDisconnect = $state<UserSource | null>(null)
    let togglingSourceId = $state<string | null>(null)

    async function toggleSource(source: UserSource, nextActive: boolean) {
        togglingSourceId = source.id
        try {
            const formData = new FormData()
            formData.append('sourceId', source.id)
            const response = await fetch(`?/${nextActive ? 'enable' : 'disable'}`, {
                method: 'POST',
                body: formData,
                headers: { 'x-sveltekit-action': 'true' },
            })
            if (!response.ok) {
                throw new Error('Failed to update source')
            }
            await invalidateAll()
        } catch (err) {
            toast.error(err instanceof Error ? err.message : 'Failed to update source')
        } finally {
            togglingSourceId = null
        }
    }

    async function confirmDisconnect() {
        const source = sourceToDisconnect
        if (!source) return
        sourceToDisconnect = null
        try {
            const response = await fetch(`/api/sources/${source.id}`, {
                method: 'DELETE',
            })
            if (!response.ok) {
                const body = await response.json().catch(() => null)
                throw new Error(body?.message || 'Failed to disconnect source')
            }
            toast.success(`${source.name} has been disconnected`)
            await invalidateAll()
        } catch (err) {
            toast.error(err instanceof Error ? err.message : 'Failed to disconnect source')
        }
    }

    let showGoogleOAuthSetup = $state(false)

    let hasGoogleDrive = $derived(data.userSources.some((s) => s.sourceType === 'google_drive'))
    let hasGmail = $derived(data.userSources.some((s) => s.sourceType === 'gmail'))
    let hasAllGoogleSources = $derived(hasGoogleDrive && hasGmail)

    function handleGoogleOAuthSetupSuccess() {
        showGoogleOAuthSetup = false
        invalidateAll()
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
                        <div class="bg-card flex items-center gap-3 rounded-lg border p-4">
                            <div
                                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm dark:border-white/10 dark:shadow-none">
                                {#if getSourceIconPath(source.sourceType)}
                                    <img
                                        src={getSourceIconPath(source.sourceType)}
                                        alt={source.name}
                                        class="h-6 w-6 object-contain" />
                                {:else if source.sourceType === 'web'}
                                    <Globe class="h-6 w-6 text-slate-700" />
                                {:else if source.sourceType === 'local_files'}
                                    <HardDrive class="h-6 w-6 text-slate-700" />
                                {:else if source.sourceType === 'imap'}
                                    <Mail class="h-6 w-6 text-slate-700" />
                                {/if}
                            </div>
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
                        <Card
                            class="group hover:border-foreground/20 flex flex-col gap-0 py-0 transition-colors">
                            <CardHeader class="flex items-center gap-3 px-4 py-4">
                                <div
                                    class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm dark:border-white/10 dark:shadow-none">
                                    {#if getSourceIconPath(source.sourceType)}
                                        <img
                                            src={getSourceIconPath(source.sourceType)}
                                            alt={source.name}
                                            class="h-6 w-6 object-contain" />
                                    {:else if source.sourceType === 'web'}
                                        <Globe class="h-6 w-6 text-slate-700" />
                                    {:else if source.sourceType === 'local_files'}
                                        <HardDrive class="h-6 w-6 text-slate-700" />
                                    {:else if source.sourceType === 'imap'}
                                        <Mail class="h-6 w-6 text-slate-700" />
                                    {/if}
                                </div>
                                <span class="truncate font-medium">{source.name}</span>
                                <Badge
                                    variant={source.isActive ? 'default' : 'secondary'}
                                    class="ml-auto">
                                    {source.isActive ? 'Enabled' : 'Paused'}
                                </Badge>
                            </CardHeader>
                            <CardFooter class="flex items-center justify-between px-4 py-2">
                                <label class="flex cursor-pointer items-center gap-2 text-sm">
                                    <Switch
                                        checked={source.isActive}
                                        disabled={togglingSourceId === source.id}
                                        onCheckedChange={(next) => toggleSource(source, next)}
                                        class="cursor-pointer" />
                                    <span class="text-muted-foreground">Sync</span>
                                </label>
                                <Button
                                    size="icon"
                                    variant="ghost"
                                    class="text-muted-foreground hover:text-destructive cursor-pointer"
                                    aria-label="Disconnect {source.name}"
                                    onclick={() => (sourceToDisconnect = source)}>
                                    <Trash2 class="h-4 w-4" />
                                </Button>
                            </CardFooter>
                        </Card>
                    {/each}
                </div>
            </div>
        {/if}

        <!-- Available Connections -->
        <!-- TODO: Move google-specific stuff out of here -->
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
                                <div
                                    class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-slate-200/70 bg-white/95 shadow-sm dark:border-white/10 dark:shadow-none">
                                    <img
                                        src={googleLogo}
                                        alt="Google"
                                        class="h-6 w-6 object-contain" />
                                </div>
                                <span>Google</span>
                                {#if hasAllGoogleSources}
                                    <span
                                        class="ml-auto inline-flex items-center rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-800 dark:bg-green-900/20 dark:text-green-400">
                                        Connected
                                    </span>
                                {/if}
                            </CardTitle>
                        </CardHeader>
                        <CardContent class="flex-1">
                            <p class="text-muted-foreground text-sm">
                                Connect your Google Drive and Gmail with read-only access. Your data
                                stays private to you.
                            </p>
                        </CardContent>
                        {#if !hasAllGoogleSources}
                            <CardFooter>
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => (showGoogleOAuthSetup = true)}>
                                    Connect with Google
                                </Button>
                            </CardFooter>
                        {/if}
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

<AlertDialog.Root
    open={sourceToDisconnect !== null}
    onOpenChange={(open) => {
        if (!open) sourceToDisconnect = null
    }}>
    <AlertDialog.Content>
        <AlertDialog.Header>
            <AlertDialog.Title>Disconnect {sourceToDisconnect?.name}?</AlertDialog.Title>
            <AlertDialog.Description>
                This will stop syncing data from this source. You can reconnect at any time.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDisconnect}>Disconnect</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>
