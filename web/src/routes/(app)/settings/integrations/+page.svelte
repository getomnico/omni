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
    import { formatDate, getSourceNoun } from '$lib/utils/sources'
    import { SourceType } from '$lib/types'
    import { invalidateAll } from '$app/navigation'
    import { toast } from 'svelte-sonner'
    import { onMount, onDestroy } from 'svelte'
    import type { SyncRun } from '$lib/server/db/schema'

    let { data }: PageProps = $props()

    type UserSource = (typeof data.userSources)[number]

    let sourceToDisconnect = $state<UserSource | null>(null)
    let togglingSourceId = $state<string | null>(null)

    type SourceId = string
    type SyncStatusPayload = {
        overall?: {
            latestSyncRuns?: SyncRun[]
            documentCounts?: Record<SourceId, number>
        }
    }

    let latestSyncRuns = $state<Map<SourceId, SyncRun>>(data.latestSyncRuns)
    let documentCounts = $state<Record<SourceId, number>>(data.documentCounts)
    let eventSource = $state<EventSource | null>(null)

    $effect(() => {
        latestSyncRuns = data.latestSyncRuns
    })

    onMount(() => {
        eventSource = new EventSource('/api/indexing/status?scope=user')
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
            } catch {
                // Silently ignore — SSE is best-effort and user already has initial load data
            }
        }

        eventSource.onerror = () => {
            // Silently ignore — SSE is best-effort
        }
    })

    onDestroy(() => {
        if (eventSource) {
            eventSource.close()
        }
    })

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
    <title>My Integrations - Settings</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <!-- Page Header -->
        <div>
            <h1 class="text-3xl font-bold tracking-tight">My Integrations</h1>
            <p class="text-muted-foreground mt-2">Your personal account connections.</p>
        </div>

        <!-- User's Own Sources -->
        {#if data.userSources.length > 0}
            <div class="space-y-4">
                <div>
                    <h2 class="text-xl font-semibold">Personal Connections</h2>
                    <p class="text-muted-foreground text-sm">Accounts connected only for you.</p>
                </div>
                <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                    {#each data.userSources as source}
                        {@const noun = getSourceNoun(source.sourceType as SourceType)}
                        {@const sync = latestSyncRuns.get(source.id)}
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
                                <div class="flex min-w-0 flex-col gap-0.5">
                                    <div class="flex items-center gap-2">
                                        <span class="truncate font-medium">{source.name}</span>
                                        <Badge
                                            variant={source.isActive ? 'default' : 'secondary'}
                                            class="ml-auto shrink-0">
                                            {source.isActive ? 'Enabled' : 'Paused'}
                                        </Badge>
                                    </div>
                                    <div
                                        class="text-muted-foreground flex items-center gap-1 text-xs">
                                        {#if sync?.status === 'running'}
                                            {#if sync.documentsScanned && sync.documentsScanned > 0}
                                                <span>
                                                    Syncing... {sync.documentsScanned.toLocaleString()}
                                                    {noun} scanned
                                                    {#if sync.documentsUpdated && sync.documentsUpdated > 0}
                                                        , {sync.documentsUpdated.toLocaleString()} updated
                                                    {/if}
                                                    {#if documentCounts[source.id]}
                                                        ({documentCounts[
                                                            source.id
                                                        ].toLocaleString()} indexed, scanned includes
                                                        duplicates across users)
                                                    {/if}
                                                </span>
                                            {:else}
                                                <span>Syncing...</span>
                                            {/if}
                                        {:else}
                                            <span
                                                >Last sync: {formatDate(
                                                    sync?.completedAt ?? null,
                                                )}</span>
                                        {/if}
                                        {#if !sync || sync.status !== 'running'}
                                            {#if documentCounts[source.id]}
                                                <span class="text-muted-foreground">·</span>
                                                <span
                                                    >{documentCounts[source.id].toLocaleString()}
                                                    {noun} indexed</span>
                                            {/if}
                                        {/if}
                                    </div>
                                </div>
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
                    <h2 class="text-xl font-semibold">Available Integrations</h2>
                    <p class="text-muted-foreground text-sm">Connect your own accounts.</p>
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
                                Connect your own Google Drive and Gmail with read-only access.
                            </p>
                        </CardContent>
                        {#if !hasAllGoogleSources}
                            <CardFooter>
                                <Button
                                    size="sm"
                                    class="cursor-pointer"
                                    onclick={() => (showGoogleOAuthSetup = true)}>
                                    Connect your Google account
                                </Button>
                            </CardFooter>
                        {/if}
                    </Card>
                </div>
            </div>
        {:else if data.userSources.length === 0}
            <div class="py-12 text-center">
                <p class="text-muted-foreground text-sm">
                    Personal integrations are not available yet. Contact your administrator.
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
                This will stop syncing this personal source. You can reconnect at any time.
            </AlertDialog.Description>
        </AlertDialog.Header>
        <AlertDialog.Footer>
            <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
            <AlertDialog.Action onclick={confirmDisconnect}>Disconnect</AlertDialog.Action>
        </AlertDialog.Footer>
    </AlertDialog.Content>
</AlertDialog.Root>
