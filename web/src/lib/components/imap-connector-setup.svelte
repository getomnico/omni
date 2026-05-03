<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import * as Select from '$lib/components/ui/select'
    import { AuthType, ServiceProvider, SourceType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    // Required connection fields
    let sourceName = $state('IMAP Mail')
    let host = $state('')
    let port = $state(993)
    let encryption = $state('tls')
    let username = $state('')
    let password = $state('')

    // Optional filters
    let folderAllowlistRaw = $state('')
    let folderDenylistRaw = $state(
        'Trash, Spam, Junk, Junk Email, Deleted Items, Deleted Messages, [Gmail]/Trash, [Gmail]/Spam',
    )
    let maxMessageSizeMb = $state(0)

    let isSubmitting = $state(false)

    function parseCommaSeparated(value: string): string[] {
        return value
            .split(',')
            .map((s) => s.trim())
            .filter((s) => s.length > 0)
    }

    async function handleSubmit() {
        if (!host.trim()) {
            toast.error('IMAP server host is required')
            return
        }
        if (!username.trim()) {
            toast.error('Username is required')
            return
        }
        if (!password) {
            toast.error('Password is required')
            return
        }
        if (port < 1 || port > 65535) {
            toast.error('Port must be between 1 and 65535')
            return
        }

        isSubmitting = true

        try {
            const config = {
                display_name: sourceName.trim() || undefined,
                host: host.trim(),
                port: Number(port),
                encryption,
                folder_allowlist: parseCommaSeparated(folderAllowlistRaw),
                folder_denylist: parseCommaSeparated(folderDenylistRaw),
                max_message_size: maxMessageSizeMb > 0 ? maxMessageSizeMb * 1024 * 1024 : 0,
                sync_enabled: true,
            }

            // 1. Create the source record
            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: sourceName.trim() || 'IMAP Mail',
                    sourceType: SourceType.IMAP,
                    config,
                }),
            })

            if (!sourceResponse.ok) {
                const text = await sourceResponse.text()
                throw new Error(`Failed to create IMAP source: ${text}`)
            }

            const source = await sourceResponse.json()

            // 2. Persist credentials (username + password) via the encrypted
            //    service-credentials API.  The connector reads these on every sync
            //    via sdk_client.get_credentials(source_id).
            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: ServiceProvider.IMAP,
                    authType: AuthType.BASIC_AUTH,
                    principalEmail: username.trim(),
                    credentials: {
                        username: username.trim(),
                        password,
                    },
                }),
            })

            if (!credentialsResponse.ok) {
                const text = await credentialsResponse.text()
                throw new Error(`Failed to save IMAP credentials: ${text}`)
            }

            toast.success('IMAP account connected successfully!')
            resetForm()

            if (onSuccess) {
                onSuccess()
            }
        } catch (err: any) {
            console.error('Error setting up IMAP:', err)
            toast.error(err.message || 'Failed to connect IMAP account')
        } finally {
            isSubmitting = false
        }
    }

    function resetForm() {
        sourceName = 'IMAP Mail'
        host = ''
        port = 993
        encryption = 'tls'
        username = ''
        password = ''
        folderAllowlistRaw = ''
        folderDenylistRaw =
            'Trash, Spam, Junk, Junk Email, Deleted Items, Deleted Messages, [Gmail]/Trash, [Gmail]/Spam'
        maxMessageSizeMb = 0
    }

    function handleCancel() {
        resetForm()
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Connect IMAP Account</Dialog.Title>
            <Dialog.Description>
                Index emails from any IMAP-compatible mailbox (Gmail, Outlook, Fastmail, etc.).
                Credentials are stored encrypted and never leave the server.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <!-- Source name -->
            <div class="space-y-1.5">
                <Label for="imap-name">Account name</Label>
                <Input
                    id="imap-name"
                    bind:value={sourceName}
                    placeholder="e.g. Work email"
                    disabled={isSubmitting} />
            </div>

            <!-- Server settings -->
            <div class="grid grid-cols-3 gap-3">
                <div class="col-span-2 space-y-1.5">
                    <Label for="imap-host">IMAP server</Label>
                    <Input
                        id="imap-host"
                        bind:value={host}
                        placeholder="imap.example.com"
                        disabled={isSubmitting}
                        required />
                </div>
                <div class="space-y-1.5">
                    <Label for="imap-port">Port</Label>
                    <Input
                        id="imap-port"
                        type="number"
                        bind:value={port}
                        min={1}
                        max={65535}
                        disabled={isSubmitting} />
                </div>
            </div>

            <!-- Encryption -->
            <div class="space-y-1.5">
                <Label for="imap-encryption">Encryption</Label>
                <Select.Root type="single" bind:value={encryption}>
                    <Select.Trigger id="imap-encryption" class="w-full" disabled={isSubmitting}>
                        {#if encryption === 'tls'}TLS / SSL (recommended){:else if encryption === 'starttls'}STARTTLS{:else}None
                            (plaintext){/if}
                    </Select.Trigger>
                    <Select.Content>
                        <Select.Item value="tls">TLS / SSL (recommended, port 993)</Select.Item>
                        <Select.Item value="starttls">STARTTLS (port 143)</Select.Item>
                        <Select.Item value="none">None — plaintext (port 143)</Select.Item>
                    </Select.Content>
                </Select.Root>
                {#if encryption === 'none'}
                    <p class="text-destructive text-xs">
                        Warning: plaintext mode sends your password without encryption.
                    </p>
                {/if}
            </div>

            <!-- Credentials -->
            <div class="space-y-1.5">
                <Label for="imap-user">Username / Email</Label>
                <Input
                    id="imap-user"
                    bind:value={username}
                    placeholder="you@example.com"
                    autocomplete="username"
                    disabled={isSubmitting}
                    required />
            </div>

            <div class="space-y-1.5">
                <Label for="imap-pass">Password</Label>
                <Input
                    id="imap-pass"
                    type="password"
                    bind:value={password}
                    placeholder="Your IMAP password or app password"
                    autocomplete="current-password"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    Use an app-specific password if two-factor authentication is enabled on your
                    account.
                </p>
            </div>

            <!-- Optional: folder filters -->
            <details class="space-y-3">
                <summary
                    class="text-muted-foreground hover:text-foreground cursor-pointer text-sm select-none">
                    Advanced options (folder filters, size limit)
                </summary>

                <div class="space-y-3 pt-1">
                    <div class="space-y-1.5">
                        <Label for="imap-allowlist"
                            >Only sync these folders (comma-separated)</Label>
                        <Input
                            id="imap-allowlist"
                            bind:value={folderAllowlistRaw}
                            placeholder="INBOX, Work, Archive (leave blank for all)"
                            disabled={isSubmitting} />
                    </div>

                    <div class="space-y-1.5">
                        <Label for="imap-denylist"
                            >Never sync these folders (comma-separated)</Label>
                        <Input
                            id="imap-denylist"
                            bind:value={folderDenylistRaw}
                            placeholder="Spam, Trash, Junk"
                            disabled={isSubmitting} />
                        <p class="text-muted-foreground text-xs">
                            These folders are excluded from indexing. The defaults cover common
                            Trash and Spam folder names across Gmail, Outlook, and other providers.
                        </p>
                    </div>

                    <div class="space-y-1.5">
                        <Label for="imap-maxsize"
                            >Skip messages larger than (MB, 0 = no limit)</Label>
                        <Input
                            id="imap-maxsize"
                            type="number"
                            bind:value={maxMessageSizeMb}
                            min={0}
                            disabled={isSubmitting} />
                    </div>
                </div>
            </details>
        </div>

        <Dialog.Footer>
            <Button
                variant="outline"
                onclick={handleCancel}
                disabled={isSubmitting}
                class="cursor-pointer">
                Cancel
            </Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting…' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
