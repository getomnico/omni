<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

    let tenantId = $state('')
    let clientId = $state('')
    let clientSecret = $state('')
    let isSubmitting = $state(false)

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

            const credentials = {
                tenant_id: tenantId.trim(),
                client_id: clientId.trim(),
                client_secret: clientSecret.trim(),
            }

            for (const { name, sourceType } of microsoftSources) {
                await createSourceWithCredentials(name, sourceType, credentials)
            }

            toast.success('Microsoft 365 connected successfully!')
            open = false

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
        open = false
        tenantId = ''
        clientId = ''
        clientSecret = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Microsoft 365</Dialog.Title>
            <Dialog.Description>
                Set up your Microsoft 365 integration using an Azure AD app registration. This will
                connect OneDrive, Outlook, Outlook Calendar, SharePoint, and Teams.
            </Dialog.Description>
        </Dialog.Header>

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
