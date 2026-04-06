<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { ArrowLeft, AlertCircle, Loader2, Cloud, Trash2 } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type { NextcloudSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const cfg = (data.source.config as Partial<NextcloudSourceConfig>) ?? {}

    // ── Connection settings ────────────────────────────────────────────────
    let enabled = $state(data.source.isActive)
    let serverUrl = $state(cfg.server_url ?? '')
    let basePath = $state(cfg.base_path ?? '/')

    // ── Credentials ────────────────────────────────────────────────────────
    let username = $state(data.principalEmail ?? '')
    let password = $state('')

    // ── File filters ───────────────────────────────────────────────────────
    let extensionAllowlist = $state(
        Array.isArray(cfg.extension_allowlist) ? cfg.extension_allowlist.join(', ') : '',
    )
    let extensionDenylist = $state(
        Array.isArray(cfg.extension_denylist) ? cfg.extension_denylist.join(', ') : '',
    )

    // ── File size ──────────────────────────────────────────────────────────
    let maxFileSizeMb = $state(
        cfg.max_file_size && cfg.max_file_size > 0
            ? Math.round(cfg.max_file_size / (1024 * 1024))
            : 0,
    )

    // ── UI state ───────────────────────────────────────────────────────────
    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let showRemoveDialog = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    // Originals for dirty-tracking
    const origEnabled = data.source.isActive
    const origServerUrl = cfg.server_url ?? ''
    const origBasePath = cfg.base_path ?? '/'
    const origUsername = data.principalEmail ?? ''
    const origAllowlist = Array.isArray(cfg.extension_allowlist)
        ? cfg.extension_allowlist.join(', ')
        : ''
    const origDenylist = Array.isArray(cfg.extension_denylist)
        ? cfg.extension_denylist.join(', ')
        : ''
    const origMaxSizeMb =
        cfg.max_file_size && cfg.max_file_size > 0
            ? Math.round(cfg.max_file_size / (1024 * 1024))
            : 0

    $effect(() => {
        hasUnsavedChanges =
            enabled !== origEnabled ||
            serverUrl !== origServerUrl ||
            basePath !== origBasePath ||
            username !== origUsername ||
            password !== '' ||
            extensionAllowlist !== origAllowlist ||
            extensionDenylist !== origDenylist ||
            maxFileSizeMb !== origMaxSizeMb
    })

    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }
        window.addEventListener('beforeunload', beforeUnloadHandler)
        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) cancel()
        }
    })

    function validateForm(): boolean {
        formErrors = []
        if (!serverUrl.trim()) {
            formErrors = [...formErrors, 'Nextcloud server URL is required']
        }
        if (!username.trim()) {
            formErrors = [...formErrors, 'Username is required']
        }
        return formErrors.length === 0
    }
</script>

<svelte:head>
    <title>Configure Nextcloud - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-4">
        <a
            href="/admin/settings/integrations"
            class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors">
            <ArrowLeft class="h-4 w-4" />
            Back to Integrations
        </a>

        {#if formErrors.length > 0}
            <Alert.Root variant="destructive">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>Configuration Error</Alert.Title>
                <Alert.Description>
                    <ul class="list-inside list-disc">
                        {#each formErrors as err}
                            <li>{err}</li>
                        {/each}
                    </ul>
                </Alert.Description>
            </Alert.Root>
        {/if}

        <form
            method="POST"
            use:enhance={() => {
                if (!validateForm()) return async () => {}
                isSubmitting = true
                return async ({ result, update }) => {
                    if (result.type === 'redirect') {
                        skipUnsavedCheck = true
                        hasUnsavedChanges = false
                        if (beforeUnloadHandler) {
                            window.removeEventListener('beforeunload', beforeUnloadHandler)
                            beforeUnloadHandler = null
                        }
                    }
                    await update()
                    isSubmitting = false
                }
            }}>
            <Card.Root>
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <Cloud class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index files from a Nextcloud instance via WebDAV (read-only)
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="enabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="enabled"
                                bind:checked={enabled}
                                name="enabled"
                                class="cursor-pointer" />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content class="space-y-6">
                    <!-- ── Connection ──────────────────────────────────── -->
                    <div class="space-y-4">
                        <h3 class="text-sm font-semibold">Connection</h3>

                        <div class="space-y-1.5">
                            <Label for="serverUrl">Nextcloud server URL</Label>
                            <Input
                                id="serverUrl"
                                name="serverUrl"
                                bind:value={serverUrl}
                                placeholder="https://cloud.example.com"
                                required />
                            <p class="text-muted-foreground text-xs">
                                The base URL of your Nextcloud instance (without /remote.php).
                            </p>
                        </div>

                        <div class="space-y-1.5">
                            <Label for="basePath">Base path</Label>
                            <Input
                                id="basePath"
                                name="basePath"
                                bind:value={basePath}
                                placeholder="/ (entire file tree)" />
                            <p class="text-muted-foreground text-xs">
                                Only sync files under this path (e.g. /Documents). Leave as / for
                                everything.
                            </p>
                        </div>
                    </div>

                    <!-- ── Credentials ─────────────────────────────────── -->
                    <div class="space-y-4 border-t pt-4">
                        <h3 class="text-sm font-semibold">Credentials</h3>

                        <div class="space-y-1.5">
                            <Label for="username">Username</Label>
                            <Input
                                id="username"
                                name="username"
                                bind:value={username}
                                placeholder="your-username"
                                autocomplete="username"
                                required />
                        </div>

                        <div class="space-y-1.5">
                            <Label for="password">
                                Password
                                <span class="text-muted-foreground ml-1 font-normal">
                                    (leave blank to keep current)
                                </span>
                            </Label>
                            <Input
                                id="password"
                                name="password"
                                type="password"
                                bind:value={password}
                                placeholder="Enter a new password to update"
                                autocomplete="new-password" />
                            <p class="text-muted-foreground text-xs">
                                If two-factor authentication is enabled, create an app password in
                                your Nextcloud Security settings.
                            </p>
                        </div>
                    </div>

                    <!-- ── File filters ────────────────────────────────── -->
                    <div class="space-y-4 border-t pt-4">
                        <h3 class="text-sm font-semibold">File Filters</h3>

                        <div class="space-y-1.5">
                            <Label for="extensionAllowlist"
                                >Only sync these file extensions (comma-separated)</Label>
                            <Input
                                id="extensionAllowlist"
                                name="extensionAllowlist"
                                bind:value={extensionAllowlist}
                                placeholder="pdf, docx, md (leave blank for all)" />
                        </div>

                        <div class="space-y-1.5">
                            <Label for="extensionDenylist"
                                >Never sync these file extensions (comma-separated)</Label>
                            <Input
                                id="extensionDenylist"
                                name="extensionDenylist"
                                bind:value={extensionDenylist}
                                placeholder="tmp, log, bak" />
                            <p class="text-muted-foreground text-xs">
                                Denylist takes priority over the allowlist.
                            </p>
                        </div>
                    </div>

                    <!-- ── File size limit ─────────────────────────────── -->
                    <div class="space-y-2 border-t pt-4">
                        <h3 class="text-sm font-semibold">File Size Limit</h3>
                        <Label for="maxFileSizeMb">Skip files larger than (MB)</Label>
                        <Input
                            id="maxFileSizeMb"
                            name="maxFileSizeMb"
                            type="number"
                            bind:value={maxFileSizeMb}
                            min={0}
                            class="w-40" />
                        <p class="text-muted-foreground text-xs">
                            Set to 0 for no limit. Useful to skip very large binary files.
                        </p>
                    </div>
                </Card.Content>

                <Card.Footer class="flex justify-end">
                    <Button
                        type="submit"
                        disabled={isSubmitting || !hasUnsavedChanges}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                        {/if}
                        Save Configuration
                    </Button>
                </Card.Footer>
            </Card.Root>
        </form>

        <!-- ── Delete source ──────────────────────────────────────────── -->
        <Card.Root>
            <Card.Content class="flex items-center justify-between">
                <div>
                    <Card.Title>Delete Source</Card.Title>
                    <Card.Description>
                        Permanently delete this source and all its synced files, credentials, and
                        sync history.
                    </Card.Description>
                </div>
                <Button
                    variant="destructive"
                    class="cursor-pointer"
                    onclick={() => (showRemoveDialog = true)}>
                    <Trash2 class="mr-2 h-4 w-4" />
                    Delete Permanently
                </Button>
            </Card.Content>
        </Card.Root>

        <RemoveSourceDialog
            bind:open={showRemoveDialog}
            sourceId={data.source.id}
            sourceName={data.source.name} />
    </div>
</div>
