<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { toast } from 'svelte-sonner'
    import { invalidateAll } from '$app/navigation'

    interface Props {
        open: boolean
        provider: string
        displayName: string
        configured?: boolean
        config?: Record<string, unknown>
        onSaved?: () => void
        onCancel?: () => void
    }

    let {
        open = false,
        provider,
        displayName,
        configured = false,
        config = {},
        onSaved,
        onCancel,
    }: Props = $props()

    let clientIdOverride = $state<string | null>(null)
    let clientSecret = $state('')
    let registrationInitialAccessToken = $state('')
    let isSaving = $state(false)

    const isSalesforceProvider = $derived(
        provider === 'salesforce' || provider.startsWith('salesforce:'),
    )
    const savedClientId = $derived(
        open && typeof config.oauth_client_id === 'string' ? config.oauth_client_id : '',
    )
    const clientId = $derived(clientIdOverride ?? savedClientId)

    async function save() {
        if (!clientId.trim() && !(isSalesforceProvider && registrationInitialAccessToken.trim())) {
            toast.error('Client ID is required unless Salesforce DCR is configured')
            return
        }
        if (
            !configured &&
            !clientSecret.trim() &&
            !(isSalesforceProvider && registrationInitialAccessToken.trim())
        ) {
            toast.error('Client secret is required unless Salesforce DCR is configured')
            return
        }

        isSaving = true
        try {
            const nextConfig: Record<string, unknown> = { ...config }
            delete nextConfig.oauth_client_secret
            if (clientId.trim()) nextConfig.oauth_client_id = clientId.trim()
            else delete nextConfig.oauth_client_id
            if (clientSecret.trim()) {
                nextConfig.oauth_client_secret = clientSecret.trim()
            }
            if (registrationInitialAccessToken.trim()) {
                nextConfig.oauth_registration_initial_access_token =
                    registrationInitialAccessToken.trim()
            }

            const response = await fetch('/api/connector-configs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ provider, config: nextConfig }),
            })

            if (!response.ok) {
                const body = await response.json().catch(() => null)
                throw new Error(body?.message || 'Failed to save OAuth client')
            }

            toast.success(`${displayName} OAuth client saved`)
            await invalidateAll()
            onSaved?.()
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Failed to save OAuth client')
        } finally {
            isSaving = false
        }
    }

    function cancel() {
        clientIdOverride = null
        clientSecret = ''
        registrationInitialAccessToken = ''
        onCancel?.()
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && cancel()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>{configured ? 'Edit' : 'Add'} {displayName} OAuth client</Dialog.Title>
            <Dialog.Description>
                Paste the OAuth app client credentials for this integration provider.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="oauth-client-id">Client ID</Label>
                <Input
                    id="oauth-client-id"
                    value={clientId}
                    oninput={(event) =>
                        (clientIdOverride = (event.currentTarget as HTMLInputElement).value)}
                    placeholder={`Enter ${displayName} client ID`} />
            </div>

            {#if isSalesforceProvider}
                <div class="space-y-2">
                    <Label for="oauth-registration-token"
                        >DCR initial access token (optional)</Label>
                    <Input
                        id="oauth-registration-token"
                        type="password"
                        bind:value={registrationInitialAccessToken}
                        placeholder="Required only when enabling Salesforce DCR" />
                    <p class="text-muted-foreground text-xs">
                        Salesforce Dynamic Client Registration requires an administrator-issued
                        initial access token. Leave blank when using a pre-created External Client
                        App.
                    </p>
                </div>
            {/if}

            <div class="space-y-2">
                <Label for="oauth-client-secret">Client secret</Label>
                <Input
                    id="oauth-client-secret"
                    type="password"
                    bind:value={clientSecret}
                    placeholder={configured
                        ? 'Leave blank to keep existing secret'
                        : `Enter ${displayName} client secret`} />
                {#if configured}
                    <p class="text-muted-foreground text-xs">
                        The existing secret is stored securely and is not shown. Enter a new secret
                        only if you want to rotate it.
                    </p>
                {/if}
            </div>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={cancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={save} disabled={isSaving} class="cursor-pointer">
                {isSaving ? 'Saving...' : 'Save'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
