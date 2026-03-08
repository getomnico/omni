<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import * as Select from '$lib/components/ui/select'
    import { ArrowLeft, AlertCircle, Loader2, Mail, X, Trash2 } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type { ImapSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const cfg = (data.source.config as Partial<ImapSourceConfig>) ?? {}

    // ── Connection settings ────────────────────────────────────────────────
    let enabled = $state(data.source.isActive)
    let host = $state(cfg.host ?? '')
    let port = $state(cfg.port ?? 993)
    let encryption = $state(cfg.encryption ?? 'tls')

    // ── Credentials ────────────────────────────────────────────────────────
    // principalEmail is the stored username (not sensitive).
    // password is always blank; the user only fills it in to change it.
    let username = $state(data.principalEmail ?? '')
    let password = $state('')

    // ── Folder filters ─────────────────────────────────────────────────────
    let folderAllowlist = $state<string[]>(
        Array.isArray(cfg.folder_allowlist) ? [...cfg.folder_allowlist] : [],
    )
    let folderDenylist = $state<string[]>(
        Array.isArray(cfg.folder_denylist) ? [...cfg.folder_denylist] : [],
    )
    let allowlistInput = $state('')
    let denylistInput = $state('')

    // ── Message size ───────────────────────────────────────────────────────
    let maxMessageSizeMb = $state(
        cfg.max_message_size && cfg.max_message_size > 0
            ? Math.round(cfg.max_message_size / (1024 * 1024))
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
    const origHost = cfg.host ?? ''
    const origPort = cfg.port ?? 993
    const origEncryption = cfg.encryption ?? 'tls'
    const origUsername = data.principalEmail ?? ''
    const origAllowlist = Array.isArray(cfg.folder_allowlist) ? [...cfg.folder_allowlist] : []
    const origDenylist = Array.isArray(cfg.folder_denylist) ? [...cfg.folder_denylist] : []
    const origMaxSizeMb =
        cfg.max_message_size && cfg.max_message_size > 0
            ? Math.round(cfg.max_message_size / (1024 * 1024))
            : 0

    $effect(() => {
        hasUnsavedChanges =
            enabled !== origEnabled ||
            host !== origHost ||
            port !== origPort ||
            encryption !== origEncryption ||
            username !== origUsername ||
            password !== '' ||
            JSON.stringify([...folderAllowlist].sort()) !==
                JSON.stringify([...origAllowlist].sort()) ||
            JSON.stringify([...folderDenylist].sort()) !==
                JSON.stringify([...origDenylist].sort()) ||
            maxMessageSizeMb !== origMaxSizeMb
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
        if (!host.trim()) {
            formErrors = [...formErrors, 'IMAP server host is required']
        }
        if (port < 1 || port > 65535) {
            formErrors = [...formErrors, 'Port must be between 1 and 65535']
        }
        if (!username.trim()) {
            formErrors = [...formErrors, 'Username is required']
        }
        return formErrors.length === 0
    }

    // ── Folder-tag helpers ─────────────────────────────────────────────────
    function addAllowlistFolder() {
        const f = allowlistInput.trim()
        if (f && !folderAllowlist.includes(f)) {
            folderAllowlist = [...folderAllowlist, f]
            allowlistInput = ''
        }
    }

    function removeAllowlistFolder(f: string) {
        folderAllowlist = folderAllowlist.filter((x) => x !== f)
    }

    function addDenylistFolder() {
        const f = denylistInput.trim()
        if (f && !folderDenylist.includes(f)) {
            folderDenylist = [...folderDenylist, f]
            denylistInput = ''
        }
    }

    function removeDenylistFolder(f: string) {
        folderDenylist = folderDenylist.filter((x) => x !== f)
    }
</script>

<svelte:head>
    <title>Configure IMAP - {data.source.name}</title>
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
                <!-- ── Header ──────────────────────────────────────────── -->
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <Mail class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index emails from an IMAP-compatible mailbox
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

                        <div class="grid grid-cols-3 gap-3">
                            <div class="col-span-2 space-y-1.5">
                                <Label for="host">IMAP server</Label>
                                <Input
                                    id="host"
                                    name="host"
                                    bind:value={host}
                                    placeholder="imap.example.com"
                                    required />
                            </div>
                            <div class="space-y-1.5">
                                <Label for="port">Port</Label>
                                <Input
                                    id="port"
                                    name="port"
                                    type="number"
                                    bind:value={port}
                                    min={1}
                                    max={65535} />
                            </div>
                        </div>

                        <div class="space-y-1.5">
                            <Label for="encryption">Encryption</Label>
                            <Select.Root type="single" bind:value={encryption} name="encryption">
                                <Select.Trigger id="encryption" class="w-full">
                                    {#if encryption === 'tls'}TLS / SSL (recommended){:else if encryption === 'starttls'}STARTTLS{:else}None (plaintext){/if}
                                </Select.Trigger>
                                <Select.Content>
                                    <Select.Item value="tls"
                                        >TLS / SSL (recommended, port 993)</Select.Item>
                                    <Select.Item value="starttls">STARTTLS (port 143)</Select.Item>
                                    <Select.Item value="none"
                                        >None — plaintext (port 143)</Select.Item>
                                </Select.Content>
                            </Select.Root>
                            {#if encryption === 'none'}
                                <p class="text-destructive text-xs">
                                    Warning: plaintext mode transmits your password without
                                    encryption.
                                </p>
                            {/if}
                            <!-- Hidden input so the form value is always submitted -->
                            <input type="hidden" name="encryption" value={encryption} />
                        </div>
                    </div>

                    <!-- ── Credentials ─────────────────────────────────── -->
                    <div class="space-y-4 border-t pt-4">
                        <h3 class="text-sm font-semibold">Credentials</h3>

                        <div class="space-y-1.5">
                            <Label for="username">Username / Email</Label>
                            <Input
                                id="username"
                                name="username"
                                bind:value={username}
                                placeholder="you@example.com"
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
                                Use an app-specific password if two-factor authentication is enabled
                                on your account.
                            </p>
                        </div>
                    </div>

                    <!-- ── Folder filters ──────────────────────────────── -->
                    <div class="space-y-4 border-t pt-4">
                        <h3 class="text-sm font-semibold">Folder Filters</h3>

                        <!-- Allowlist -->
                        <div class="space-y-2">
                            <Label class="text-sm">Only sync these folders</Label>
                            <p class="text-muted-foreground text-xs">
                                Leave empty to sync all folders. If set, only listed folders are
                                indexed.
                            </p>
                            <div class="flex gap-2">
                                <Input
                                    bind:value={allowlistInput}
                                    placeholder="e.g. INBOX"
                                    class="flex-1"
                                    onkeydown={(e) => {
                                        if (e.key === 'Enter') {
                                            e.preventDefault()
                                            addAllowlistFolder()
                                        }
                                    }} />
                                <Button
                                    type="button"
                                    variant="secondary"
                                    onclick={addAllowlistFolder}
                                    disabled={!allowlistInput.trim()}>
                                    Add
                                </Button>
                            </div>
                            {#if folderAllowlist.length > 0}
                                <div class="flex flex-wrap gap-2">
                                    {#each folderAllowlist as folder}
                                        <input
                                            type="hidden"
                                            name="folderAllowlist"
                                            value={folder} />
                                        <div
                                            class="bg-secondary text-secondary-foreground inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium">
                                            <span>{folder}</span>
                                            <button
                                                type="button"
                                                onclick={() => removeAllowlistFolder(folder)}
                                                class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5"
                                                aria-label="Remove {folder}">
                                                <X class="h-3 w-3" />
                                            </button>
                                        </div>
                                    {/each}
                                </div>
                            {/if}
                        </div>

                        <!-- Denylist -->
                        <div class="space-y-2 border-t pt-2">
                            <Label class="text-sm">Never sync these folders</Label>
                            <p class="text-muted-foreground text-xs">
                                Denylist takes priority over the allowlist.
                            </p>
                            <div class="flex gap-2">
                                <Input
                                    bind:value={denylistInput}
                                    placeholder="e.g. Spam, Trash"
                                    class="flex-1"
                                    onkeydown={(e) => {
                                        if (e.key === 'Enter') {
                                            e.preventDefault()
                                            addDenylistFolder()
                                        }
                                    }} />
                                <Button
                                    type="button"
                                    variant="secondary"
                                    onclick={addDenylistFolder}
                                    disabled={!denylistInput.trim()}>
                                    Add
                                </Button>
                            </div>
                            {#if folderDenylist.length > 0}
                                <div class="flex flex-wrap gap-2">
                                    {#each folderDenylist as folder}
                                        <input
                                            type="hidden"
                                            name="folderDenylist"
                                            value={folder} />
                                        <div
                                            class="bg-secondary text-secondary-foreground inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium">
                                            <span>{folder}</span>
                                            <button
                                                type="button"
                                                onclick={() => removeDenylistFolder(folder)}
                                                class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5"
                                                aria-label="Remove {folder}">
                                                <X class="h-3 w-3" />
                                            </button>
                                        </div>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    </div>

                    <!-- ── Message size limit ──────────────────────────── -->
                    <div class="space-y-2 border-t pt-4">
                        <h3 class="text-sm font-semibold">Message Size Limit</h3>
                        <Label for="maxMessageSizeMb">Skip messages larger than (MB)</Label>
                        <Input
                            id="maxMessageSizeMb"
                            name="maxMessageSizeMb"
                            type="number"
                            bind:value={maxMessageSizeMb}
                            min={0}
                            class="w-40" />
                        <p class="text-muted-foreground text-xs">
                            Set to 0 for no limit. Useful to skip large attachments.
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
                        Permanently delete this source and all its synced emails, credentials, and
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
