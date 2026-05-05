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
                    scope: 'org',
                    name: 'Fireflies',
                    sourceType: 'fireflies',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Fireflies source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'fireflies',
                    authType: AuthType.API_KEY,
                    credentials: { api_key: apiKey },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Fireflies service credentials')
            }

            toast.success('Fireflies connected successfully!')

            apiKey = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Fireflies:', error)
            toast.error(error.message || 'Failed to set up Fireflies')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        apiKey = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Fireflies</Dialog.Title>
            <Dialog.Description>
                Set up your Fireflies.ai integration to index meeting transcripts, summaries, and
                action items.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="api-key">API Key</Label>
                <Input
                    id="api-key"
                    bind:value={apiKey}
                    placeholder="Your Fireflies API key"
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Get your API key from <a
                        href="https://app.fireflies.ai/integrations/custom/fireflies"
                        target="_blank"
                        class="text-blue-600 hover:underline">Fireflies Integrations settings</a>
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
