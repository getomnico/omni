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

    let apiKey = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!apiKey.trim()) {
                throw new Error('API key is required')
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Linear',
                    sourceType: 'linear',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Linear source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'linear',
                    authType: AuthType.API_KEY,
                    credentials: { api_key: apiKey },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Linear service credentials')
            }

            toast.success('Linear connected successfully!')
            open = false

            apiKey = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Linear:', error)
            toast.error(error.message || 'Failed to set up Linear')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        apiKey = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Linear</Dialog.Title>
            <Dialog.Description>
                Set up your Linear integration to index issues, projects, and documents.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="api-key">API Key</Label>
                <Input
                    id="api-key"
                    bind:value={apiKey}
                    placeholder="lin_api_..."
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create a personal API key at <a
                        href="https://linear.app/settings/api"
                        target="_blank"
                        class="text-blue-600 hover:underline">Linear API settings</a>
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
