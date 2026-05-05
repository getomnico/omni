<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    let token = $state('')
    let includeDocs = $state(true)
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!token.trim()) {
                throw new Error('API token is required')
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'ClickUp',
                    sourceType: 'clickup',
                    config: { include_docs: includeDocs },
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create ClickUp source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'clickup',
                    authType: AuthType.API_KEY,
                    credentials: { token },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create ClickUp service credentials')
            }

            toast.success('ClickUp connected successfully!')

            token = ''
            includeDocs = true

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up ClickUp:', error)
            toast.error(error.message || 'Failed to set up ClickUp')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        token = ''
        includeDocs = true
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect ClickUp</Dialog.Title>
            <Dialog.Description>
                Set up your ClickUp integration to index tasks and docs.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="clickup-token">API Token</Label>
                <Input
                    id="clickup-token"
                    bind:value={token}
                    placeholder="pk_..."
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Get your personal API token from ClickUp
                    <a
                        href="https://app.clickup.com/settings/apps"
                        target="_blank"
                        class="text-blue-600 hover:underline">Settings &rarr; Apps</a>
                </p>
            </div>

            <div class="flex items-center gap-2">
                <Checkbox id="include-docs" bind:checked={includeDocs} class="cursor-pointer" />
                <Label for="include-docs" class="cursor-pointer">Include Docs</Label>
                <p class="text-muted-foreground text-sm">
                    Also index ClickUp Docs in addition to tasks
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
