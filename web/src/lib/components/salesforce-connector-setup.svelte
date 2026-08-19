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

    let { open = false, onSuccess, onCancel }: Props = $props()

    let instanceUrl = $state('')
    let accessToken = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!instanceUrl.trim()) {
                throw new Error('Instance URL is required')
            }

            if (!accessToken.trim()) {
                throw new Error('Access token is required')
            }

            // Normalize instance URL
            let normalizedUrl = instanceUrl.trim()
            if (!normalizedUrl.startsWith('https://')) {
                normalizedUrl = `https://${normalizedUrl}`
            }
            normalizedUrl = normalizedUrl.replace(/\/+$/, '')

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Salesforce',
                    sourceType: 'salesforce',
                    config: { instance_url: normalizedUrl },
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Salesforce source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'salesforce',
                    authType: AuthType.BEARER_TOKEN,
                    credentials: {
                        access_token: accessToken,
                        instance_url: normalizedUrl,
                    },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Salesforce service credentials')
            }

            toast.success('Salesforce connected successfully!')

            instanceUrl = ''
            accessToken = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Salesforce:', error)
            toast.error(error.message || 'Failed to set up Salesforce')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        instanceUrl = ''
        accessToken = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Salesforce</Dialog.Title>
            <Dialog.Description>
                Set up your Salesforce integration using an access token and instance URL.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="instance-url">Instance URL</Label>
                <Input
                    id="instance-url"
                    bind:value={instanceUrl}
                    placeholder="https://yourorg.salesforce.com"
                    required />
                <p class="text-muted-foreground text-sm">
                    Your Salesforce instance URL (e.g., https://yourorg.salesforce.com)
                </p>
            </div>

            <div class="space-y-2">
                <Label for="access-token">Access Token</Label>
                <Input
                    id="access-token"
                    bind:value={accessToken}
                    placeholder="Your Salesforce access token"
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Get an access token from Setup &gt; Apps &gt; Connected Apps, or use a session
                    ID
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
