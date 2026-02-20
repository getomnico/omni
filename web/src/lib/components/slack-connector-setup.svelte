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

    let botToken = $state('')
    let appToken = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!botToken.trim()) {
                throw new Error('Bot token is required')
            }

            if (!botToken.startsWith('xoxb-')) {
                throw new Error('Bot token must start with xoxb-')
            }

            if (appToken.trim() && !appToken.startsWith('xapp-')) {
                throw new Error('App-Level Token must start with xapp-')
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Slack',
                    sourceType: 'slack',
                    config: {},
                }),
            })

            if (!sourceResponse.ok) {
                throw new Error('Failed to create Slack source')
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'slack',
                    authType: AuthType.BOT_TOKEN,
                    credentials: {
                        bot_token: botToken,
                        ...(appToken.trim() ? { app_token: appToken } : {}),
                    },
                }),
            })

            if (!credentialsResponse.ok) {
                throw new Error('Failed to create Slack service credentials')
            }

            toast.success('Slack connected successfully!')
            open = false

            botToken = ''
            appToken = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Slack:', error)
            toast.error(error.message || 'Failed to set up Slack')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        botToken = ''
        appToken = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Slack</Dialog.Title>
            <Dialog.Description>
                Set up your Slack integration using a bot token.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="bot-token">Bot Token</Label>
                <Input
                    id="bot-token"
                    bind:value={botToken}
                    placeholder="xoxb-..."
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create a Slack app and get a bot token at <a
                        href="https://api.slack.com/apps"
                        target="_blank"
                        class="text-blue-600 hover:underline">api.slack.com/apps</a>
                </p>
            </div>

            <div class="space-y-2">
                <Label for="app-token">App-Level Token (optional)</Label>
                <Input
                    id="app-token"
                    bind:value={appToken}
                    placeholder="xapp-..."
                    type="password" />
                <p class="text-muted-foreground text-sm">
                    Enables realtime updates via Socket Mode. Generate one under your Slack App
                    &rarr; Settings &rarr; Basic Information &rarr; App-Level Tokens with the
                    <code class="bg-muted rounded px-1">connections:write</code> scope.
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
