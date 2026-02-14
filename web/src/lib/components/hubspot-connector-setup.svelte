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

    let accessToken = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!accessToken.trim()) {
                throw new Error('Access token is required')
            }

            if (!accessToken.startsWith('pat-')) {
                throw new Error('Access token must start with pat-')
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'HubSpot',
                    sourceType: 'hubspot',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create HubSpot source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'hubspot',
                    authType: AuthType.BEARER_TOKEN,
                    credentials: { access_token: accessToken },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create HubSpot service credentials')
            }

            toast.success('HubSpot connected successfully!')
            open = false

            accessToken = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up HubSpot:', error)
            toast.error(error.message || 'Failed to set up HubSpot')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        accessToken = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect HubSpot</Dialog.Title>
            <Dialog.Description>
                Set up your HubSpot integration using a private app access token.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="access-token">Access Token</Label>
                <Input
                    id="access-token"
                    bind:value={accessToken}
                    placeholder="pat-na1-..."
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create a private app and get an access token at <a
                        href="https://app.hubspot.com/private-apps/"
                        target="_blank"
                        class="text-blue-600 hover:underline">HubSpot Private Apps settings</a>
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
