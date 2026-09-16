<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType } from '$lib/types'
    import { copyTextToClipboard } from '$lib/utils'
    import { toast } from 'svelte-sonner'
    import { Check, Copy } from '@lucide/svelte'

    interface Props {
        open: boolean
        oauthRedirectUri?: string
        onSuccess?: () => void
        onCancel?: () => void
    }

    let {
        open = false,
        oauthRedirectUri = '',
        onSuccess,
        onCancel,
    }: Props = $props()

    let tenantId = $state('')
    let clientId = $state('')
    let clientSecret = $state('')
    let isSubmitting = $state(false)
    let redirectUriCopied = $state(false)
    let copyResetTimer: ReturnType<typeof setTimeout> | null = null

    const microsoftSources = [
        { name: 'OneDrive', sourceType: 'one_drive' },
        { name: 'Outlook', sourceType: 'outlook' },
        { name: 'Outlook Calendar', sourceType: 'outlook_calendar' },
        { name: 'SharePoint', sourceType: 'share_point' },
        { name: 'Teams', sourceType: 'ms_teams' },
    ] as const

    async function createSourceWithCredentials(
        name: string,
        sourceType: string,
        credentials: Record<string, string>,
    ) {
        const sourceResponse = await fetch('/api/sources', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                scope: 'org',
                name,
                sourceType,
                config: {},
            }),
        })

        if (!sourceResponse.ok) {
            throw new Error(`Failed to create ${name} source`)
        }

        const source = await sourceResponse.json()

        const credentialsResponse = await fetch('/api/service-credentials', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                sourceId: source.id,
                provider: 'microsoft',
                authType: AuthType.BEARER_TOKEN,
                credentials,
            }),
        })

        if (!credentialsResponse.ok) {
            throw new Error(`Failed to create ${name} service credentials`)
        }
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!tenantId.trim()) {
                throw new Error('Tenant ID is required')
            }
            if (!clientId.trim()) {
                throw new Error('Application (Client) ID is required')
            }
            if (!clientSecret.trim()) {
                throw new Error('Client Secret is required')
            }

            const trimmedTenant = tenantId.trim()
            const trimmedClientId = clientId.trim()
            const trimmedClientSecret = clientSecret.trim()

            const credentials = {
                tenant_id: trimmedTenant,
                client_id: trimmedClientId,
                client_secret: trimmedClientSecret,
            }

            for (const { name, sourceType } of microsoftSources) {
                await createSourceWithCredentials(name, sourceType, credentials)
            }

            // Same Azure AD app registration powers both flows: the per-source
            // app-only credentials above, and the per-user OAuth flow that
            // reads from connector_configs. Materialize tenant-specific
            // endpoints here so the generic OAuth client doesn't need to do
            // any string substitution.
            const oauthConfigResponse = await fetch('/api/connector-configs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    provider: 'microsoft',
                    config: {
                        oauth_client_id: trimmedClientId,
                        oauth_client_secret: trimmedClientSecret,
                        oauth_auth_endpoint: `https://login.microsoftonline.com/${trimmedTenant}/oauth2/v2.0/authorize`,
                        oauth_token_endpoint: `https://login.microsoftonline.com/${trimmedTenant}/oauth2/v2.0/token`,
                    },
                }),
            })

            if (!oauthConfigResponse.ok) {
                throw new Error('Failed to save Microsoft OAuth configuration')
            }

            toast.success('Microsoft 365 connected successfully!')

            tenantId = ''
            clientId = ''
            clientSecret = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Microsoft 365:', error)
            toast.error(error.message || 'Failed to set up Microsoft 365')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        tenantId = ''
        clientId = ''
        clientSecret = ''
        if (onCancel) {
            onCancel()
        }
    }

    async function copyRedirectUri() {
        if (!oauthRedirectUri) return
        try {
            await copyTextToClipboard(oauthRedirectUri)
            redirectUriCopied = true
            toast.success('Redirect URI copied')
            if (copyResetTimer) clearTimeout(copyResetTimer)
            copyResetTimer = setTimeout(() => {
                redirectUriCopied = false
                copyResetTimer = null
            }, 2000)
        } catch {
            toast.error('Failed to copy redirect URI')
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Microsoft 365</Dialog.Title>
            <Dialog.Description>
                Set up your Microsoft 365 integration using an Azure AD app registration. This will
                connect OneDrive, Outlook, Outlook Calendar, SharePoint, and Teams.
            </Dialog.Description>
        </Dialog.Header>

        <div
            class="bg-muted/50 text-muted-foreground rounded-md border p-3 text-xs leading-relaxed">
            <p>
                Grant <span class="font-medium">both</span> Application permissions (used for
                org-wide sync) and Delegated permissions (used when individual users connect
                their account for tools) on Microsoft Graph for this app registration. Omni
                reuses the same client credentials for both flows.
            </p>
            <div class="mt-2 grid gap-x-4 gap-y-1 sm:grid-cols-2">
                <div>
                    <p class="text-foreground font-medium">Application permissions</p>
                    <p>Files.Read.All, Sites.Read.All, Mail.Read, Calendars.Read,
                    User.Read.All, Chat.Read.All, ChatMessage.Read.All</p>
                </div>
                <div>
                    <p class="text-foreground font-medium">Delegated permissions</p>
                    <p>User.Read, Calendars.Read, Calendars.ReadWrite</p>
                </div>
            </div>
            {#if oauthRedirectUri}
                <div class="mt-2">
                    <p class="text-foreground font-medium">Redirect URI</p>
                    <p>
                        Add under <span class="font-medium">Authentication</span> as a
                        <span class="font-medium">Web</span> platform redirect URI — without it,
                        users hitting “Connect account” get
                        <span class="font-mono">AADSTS500113</span>.
                    </p>
                    <div class="mt-1 flex gap-2">
                        <code
                            class="bg-background text-muted-foreground flex-1 rounded border px-2 py-1 break-all">
                            {oauthRedirectUri}
                        </code>
                        <Button
                            variant="outline"
                            size="sm"
                            class="cursor-pointer"
                            onclick={copyRedirectUri}>
                            {#if redirectUriCopied}
                                <Check class="h-3.5 w-3.5 text-green-600" />
                                Copied
                            {:else}
                                <Copy class="h-3.5 w-3.5" />
                                Copy
                            {/if}
                        </Button>
                    </div>
                </div>
            {/if}
            <p class="mt-2">
                Then click <span class="font-medium">Grant admin consent</span> for the
                tenant in Azure (App registrations → API permissions). Without granted
                consent, sources authenticate but every sync fails with
                <span class="font-mono">Insufficient privileges</span>.
            </p>
        </div>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="ms-tenant-id">Tenant ID</Label>
                <Input
                    id="ms-tenant-id"
                    bind:value={tenantId}
                    placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                    required />
                <p class="text-muted-foreground text-sm">
                    Your Azure AD tenant ID. Find it in the Azure Portal under "Azure Active
                    Directory" > "Overview".
                </p>
            </div>

            <div class="space-y-2">
                <Label for="ms-client-id">Application (Client) ID</Label>
                <Input
                    id="ms-client-id"
                    bind:value={clientId}
                    placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                    required />
                <p class="text-muted-foreground text-sm">
                    The Application (client) ID from your Azure AD app registration.
                </p>
            </div>

            <div class="space-y-2">
                <Label for="ms-client-secret">Client Secret</Label>
                <Input
                    id="ms-client-secret"
                    bind:value={clientSecret}
                    type="password"
                    placeholder="Enter your client secret"
                    required />
                <p class="text-muted-foreground text-sm">
                    A client secret from your Azure AD app registration under "Certificates &
                    secrets".
                </p>
            </div>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
