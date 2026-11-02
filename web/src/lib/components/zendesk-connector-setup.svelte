<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import * as Select from '$lib/components/ui/select'
    import { AuthType, ServiceProvider, SourceType, type ZendeskSourceConfig } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    const SUBDOMAIN_PATTERN = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/i

    let sourceName = $state('Zendesk')
    let subdomain = $state('')
    type CredentialType = 'api_token' | 'access_token'

    let credentialType = $state<CredentialType>('api_token')
    let email = $state('')
    let apiToken = $state('')
    let accessToken = $state('')
    let syncTickets = $state(true)
    let syncHelpCenter = $state(true)

    let isSubmitting = $state(false)

    async function handleSubmit() {
        const trimmedSubdomain = subdomain.trim()
        const trimmedEmail = email.trim()
        if (!SUBDOMAIN_PATTERN.test(trimmedSubdomain)) {
            toast.error(
                'Subdomain must be 1-63 letters, digits or hyphens, starting and ending with a letter or digit',
            )
            return
        }
        if (credentialType === 'api_token') {
            if (!trimmedEmail) {
                toast.error('Admin email is required')
                return
            }
            if (!apiToken) {
                toast.error('API token is required')
                return
            }
        } else if (!accessToken) {
            toast.error('Access token is required')
            return
        }
        if (!syncTickets && !syncHelpCenter) {
            toast.error('Select at least one thing to sync')
            return
        }

        isSubmitting = true

        try {
            const config: ZendeskSourceConfig = {
                subdomain: trimmedSubdomain,
                sync_tickets: syncTickets,
                sync_help_center: syncHelpCenter,
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: sourceName.trim() || 'Zendesk',
                    sourceType: SourceType.ZENDESK,
                    config,
                }),
            })

            if (!sourceResponse.ok) {
                const text = await sourceResponse.text()
                throw new Error(`Failed to create Zendesk source: ${text}`)
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: ServiceProvider.ZENDESK,
                    ...(credentialType === 'api_token'
                        ? {
                              authType: AuthType.API_KEY,
                              principalEmail: trimmedEmail,
                              credentials: { email: trimmedEmail, api_token: apiToken },
                          }
                        : {
                              authType: AuthType.BEARER_TOKEN,
                              principalEmail: null,
                              credentials: { access_token: accessToken },
                          }),
                }),
            })

            if (!credentialsResponse.ok) {
                const text = await credentialsResponse.text()
                throw new Error(`Failed to save Zendesk credentials: ${text}`)
            }

            // Validate against Zendesk before declaring success; delete the source on failure.
            try {
                const validateResponse = await fetch(`/api/sources/${source.id}/action`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ action: 'validate_credentials', params: {} }),
                })
                if (validateResponse.ok) {
                    const result = await validateResponse.json()
                    if (result?.result?.authenticated === false) {
                        await fetch(`/api/sources/${source.id}`, { method: 'DELETE' })
                        throw new Error(
                            credentialType === 'api_token'
                                ? 'Authentication failed. Check the admin email and API token.'
                                : 'Authentication failed. Check the access token.',
                        )
                    }
                }
                // If the validate call itself fails (e.g. connector not yet registered),
                // continue — the user will find out on the first sync.
            } catch (validateErr) {
                if (
                    validateErr instanceof Error &&
                    validateErr.message.includes('Authentication failed')
                ) {
                    throw validateErr
                }
                console.warn('Credential validation skipped:', validateErr)
            }

            toast.success('Zendesk connected successfully!')
            resetForm()
            onSuccess?.()
        } catch (err) {
            console.error('Error setting up Zendesk:', err)
            toast.error(err instanceof Error ? err.message : 'Failed to connect Zendesk')
        } finally {
            isSubmitting = false
        }
    }

    function resetForm() {
        sourceName = 'Zendesk'
        subdomain = ''
        credentialType = 'api_token'
        email = ''
        apiToken = ''
        accessToken = ''
        syncTickets = true
        syncHelpCenter = true
    }

    function handleCancel() {
        resetForm()
        onCancel?.()
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Connect Zendesk</Dialog.Title>
            <Dialog.Description>
                Index Zendesk Support tickets and Help Center articles (read-only). Credentials are
                stored encrypted and never leave the server.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-1.5">
                <Label for="zd-name">Connection name</Label>
                <Input
                    id="zd-name"
                    bind:value={sourceName}
                    placeholder="e.g. Company Support"
                    disabled={isSubmitting} />
            </div>

            <div class="space-y-1.5">
                <Label for="zd-subdomain">Zendesk subdomain</Label>
                <Input
                    id="zd-subdomain"
                    bind:value={subdomain}
                    placeholder="acme"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    https://{subdomain.trim() || '<subdomain>'}.zendesk.com
                </p>
            </div>

            <div class="space-y-1.5">
                <Label for="zd-credential-type">Credential type</Label>
                <Select.Root type="single" bind:value={credentialType} disabled={isSubmitting}>
                    <Select.Trigger id="zd-credential-type" class="w-full cursor-pointer">
                        {credentialType === 'api_token'
                            ? 'API token'
                            : 'OAuth access token (advanced)'}
                    </Select.Trigger>
                    <Select.Content>
                        <Select.Item value="api_token" class="cursor-pointer"
                            >API token</Select.Item>
                        <Select.Item value="access_token" class="cursor-pointer"
                            >OAuth access token (advanced)</Select.Item>
                    </Select.Content>
                </Select.Root>
            </div>

            {#if credentialType === 'api_token'}
                <div class="space-y-1.5">
                    <Label for="zd-email">Admin email</Label>
                    <Input
                        id="zd-email"
                        type="email"
                        bind:value={email}
                        placeholder="admin@example.com"
                        autocomplete="username"
                        disabled={isSubmitting}
                        required />
                </div>

                <div class="space-y-1.5">
                    <Label for="zd-token">API token</Label>
                    <Input
                        id="zd-token"
                        type="password"
                        bind:value={apiToken}
                        autocomplete="new-password"
                        disabled={isSubmitting}
                        required />
                    <p class="text-muted-foreground text-xs">
                        In Admin Center go to Apps and integrations → APIs → Zendesk API, enable
                        token access and add an API token. The token must be created by an admin
                        user.
                    </p>
                </div>
            {:else}
                <div class="space-y-1.5">
                    <Label for="zd-access-token">Access token</Label>
                    <Input
                        id="zd-access-token"
                        type="password"
                        bind:value={accessToken}
                        autocomplete="new-password"
                        disabled={isSubmitting}
                        required />
                    <p class="text-muted-foreground text-xs">
                        An OAuth access token for this account with read access, belonging to an
                        admin. Omni cannot refresh it: when it expires, syncs fail until you save a
                        new token on the settings page.
                    </p>
                </div>
            {/if}

            <div class="space-y-2">
                <div class="flex items-center gap-2">
                    <Checkbox id="zd-tickets" bind:checked={syncTickets} disabled={isSubmitting} />
                    <Label for="zd-tickets" class="cursor-pointer">Sync tickets</Label>
                </div>
                <div class="flex items-center gap-2">
                    <Checkbox
                        id="zd-help-center"
                        bind:checked={syncHelpCenter}
                        disabled={isSubmitting} />
                    <Label for="zd-help-center" class="cursor-pointer"
                        >Sync Help Center articles</Label>
                </div>
            </div>
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
