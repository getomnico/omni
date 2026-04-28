<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType, ServiceProvider, SourceType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    let sourceName = $state('Nextcloud Files')
    let serverUrl = $state('')
    let username = $state('')
    let password = $state('')
    let basePath = $state('/')

    // Optional filters
    let extensionAllowlistRaw = $state('')
    let extensionDenylistRaw = $state('')
    let maxFileSizeMb = $state(0)

    let isSubmitting = $state(false)

    function parseCommaSeparated(value: string): string[] {
        return value
            .split(',')
            .map((s) => s.trim())
            .filter((s) => s.length > 0)
    }

    async function handleSubmit() {
        if (!serverUrl.trim()) {
            toast.error('Nextcloud server URL is required')
            return
        }
        if (!username.trim()) {
            toast.error('Username is required')
            return
        }
        if (!password) {
            toast.error('Password or app password is required')
            return
        }

        isSubmitting = true

        try {
            const config = {
                server_url: serverUrl.trim().replace(/\/+$/, ''),
                base_path: basePath.trim() || '/',
                extension_allowlist: parseCommaSeparated(extensionAllowlistRaw),
                extension_denylist: parseCommaSeparated(extensionDenylistRaw),
                max_file_size: maxFileSizeMb > 0 ? maxFileSizeMb * 1024 * 1024 : 0,
                sync_enabled: true,
            }

            // 1. Create the source record
            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: sourceName.trim() || 'Nextcloud Files',
                    sourceType: SourceType.NEXTCLOUD,
                    config,
                }),
            })

            if (!sourceResponse.ok) {
                const text = await sourceResponse.text()
                throw new Error(`Failed to create Nextcloud source: ${text}`)
            }

            const source = await sourceResponse.json()

            // 2. Persist credentials (username + password) via the encrypted
            //    service-credentials API.
            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: ServiceProvider.NEXTCLOUD,
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
                throw new Error(`Failed to save Nextcloud credentials: ${text}`)
            }

            // Validate credentials against the Nextcloud server before declaring success.
            // If validation fails, delete the source and report the error.
            try {
                const validateResponse = await fetch(`/api/sources/${source.id}/action`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        action: 'validate_credentials',
                        params: config,
                    }),
                })
                if (validateResponse.ok) {
                    const result = await validateResponse.json()
                    if (result?.result?.authenticated === false) {
                        // Clean up the source we just created
                        await fetch(`/api/sources/${source.id}`, { method: 'DELETE' })
                        throw new Error(
                            'Authentication failed. Please check your username and password.',
                        )
                    }
                }
                // If the validate call itself fails (e.g. connector not yet registered),
                // continue — the user will find out on the first sync.
            } catch (validateErr: any) {
                if (validateErr.message.includes('Authentication failed')) {
                    throw validateErr
                }
                console.warn('Credential validation skipped:', validateErr.message)
            }

            toast.success('Nextcloud connected successfully!')
            resetForm()

            if (onSuccess) {
                onSuccess()
            }
        } catch (err: any) {
            console.error('Error setting up Nextcloud:', err)
            toast.error(err.message || 'Failed to connect Nextcloud')
        } finally {
            isSubmitting = false
        }
    }

    function resetForm() {
        sourceName = 'Nextcloud Files'
        serverUrl = ''
        username = ''
        password = ''
        basePath = '/'
        extensionAllowlistRaw = ''
        extensionDenylistRaw = ''
        maxFileSizeMb = 0
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
            <Dialog.Title>Connect Nextcloud</Dialog.Title>
            <Dialog.Description>
                Index files from your Nextcloud instance. Uses WebDAV for read-only access.
                Credentials are stored encrypted and never leave the server.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <!-- Source name -->
            <div class="space-y-1.5">
                <Label for="nc-name">Connection name</Label>
                <Input
                    id="nc-name"
                    bind:value={sourceName}
                    placeholder="e.g. Company Nextcloud"
                    disabled={isSubmitting} />
            </div>

            <!-- Server URL -->
            <div class="space-y-1.5">
                <Label for="nc-url">Nextcloud server URL</Label>
                <Input
                    id="nc-url"
                    bind:value={serverUrl}
                    placeholder="https://cloud.example.com"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    The base URL of your Nextcloud instance (without /remote.php).
                </p>
            </div>

            <!-- Credentials -->
            <div class="space-y-1.5">
                <Label for="nc-user">Username</Label>
                <Input
                    id="nc-user"
                    bind:value={username}
                    placeholder="your-username"
                    autocomplete="username"
                    disabled={isSubmitting}
                    required />
            </div>

            <div class="space-y-1.5">
                <Label for="nc-pass">Password</Label>
                <Input
                    id="nc-pass"
                    type="password"
                    bind:value={password}
                    placeholder="Your password or app password"
                    autocomplete="current-password"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    If two-factor authentication is enabled, create an app password in your
                    Nextcloud Security settings.
                </p>
            </div>

            <!-- Optional: advanced options -->
            <details class="space-y-3">
                <summary
                    class="text-muted-foreground hover:text-foreground cursor-pointer text-sm select-none">
                    Advanced options (base path, file filters, size limit)
                </summary>

                <div class="space-y-3 pt-1">
                    <div class="space-y-1.5">
                        <Label for="nc-basepath">Base path</Label>
                        <Input
                            id="nc-basepath"
                            bind:value={basePath}
                            placeholder="/ (entire file tree)"
                            disabled={isSubmitting} />
                        <p class="text-muted-foreground text-xs">
                            Only sync files under this path (e.g. /Documents). Leave as / for
                            everything.
                        </p>
                    </div>

                    <div class="space-y-1.5">
                        <Label for="nc-allowlist"
                            >Only sync these file extensions (comma-separated)</Label>
                        <Input
                            id="nc-allowlist"
                            bind:value={extensionAllowlistRaw}
                            placeholder="pdf, docx, md (leave blank for all)"
                            disabled={isSubmitting} />
                    </div>

                    <div class="space-y-1.5">
                        <Label for="nc-denylist"
                            >Never sync these file extensions (comma-separated)</Label>
                        <Input
                            id="nc-denylist"
                            bind:value={extensionDenylistRaw}
                            placeholder="tmp, log, bak"
                            disabled={isSubmitting} />
                    </div>

                    <div class="space-y-1.5">
                        <Label for="nc-maxsize">Skip files larger than (MB, 0 = no limit)</Label>
                        <Input
                            id="nc-maxsize"
                            type="number"
                            bind:value={maxFileSizeMb}
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
