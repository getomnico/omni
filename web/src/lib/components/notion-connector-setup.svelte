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

    let token = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!token.trim()) {
                throw new Error('Integration token is required')
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Notion',
                    sourceType: 'notion',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Notion source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'notion',
                    authType: AuthType.API_KEY,
                    credentials: { token },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Notion service credentials')
            }

            toast.success('Notion connected successfully!')
            open = false

            token = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Notion:', error)
            toast.error(error.message || 'Failed to set up Notion')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        token = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Notion</Dialog.Title>
            <Dialog.Description>
                Set up your Notion integration to index pages and databases.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="notion-token">Internal Integration Token</Label>
                <Input
                    id="notion-token"
                    bind:value={token}
                    placeholder="ntn_..."
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create an internal integration at
                    <a
                        href="https://www.notion.so/profile/integrations"
                        target="_blank"
                        class="text-blue-600 hover:underline">Notion Integrations</a>
                    and paste the token here.
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
