<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'

    interface Props {
        open: boolean
        baseUrl?: string | null
        onCancel?: () => void
    }

    let { open = false, baseUrl = null, onCancel }: Props = $props()
    let isConnecting = $state(false)

    async function handleConnect() {
        isConnecting = true
        const returnTo = encodeURIComponent('/settings/integrations?success=connected')
        window.location.href = `/api/oauth/start?source_types=windshift&return_to=${returnTo}`
    }
</script>

<Dialog.Root {open} onOpenChange={(value) => !value && onCancel?.()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Connect Windshift</Dialog.Title>
            <Dialog.Description>
                Connect your Windshift account and index the workspaces you can access.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-3 py-2">
            {#if baseUrl}
                <div class="space-y-1">
                    <div class="text-sm font-medium">Windshift URL</div>
                    <code class="bg-muted block rounded-md px-3 py-2 text-sm break-all">
                        {baseUrl}
                    </code>
                </div>
            {/if}
            <p class="text-sm">
                Windshift will show the exact read permissions Omni needs. The OAuth client is
                registered automatically and uses PKCE; there is no client secret to configure.
            </p>
            <p class="text-muted-foreground text-xs">
                Indexed content is private to your Omni account. Write actions request expanded
                authorization when they are first used.
            </p>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={() => onCancel?.()} class="cursor-pointer">
                Cancel
            </Button>
            <Button onclick={handleConnect} disabled={isConnecting} class="cursor-pointer">
                {isConnecting ? 'Connecting...' : 'Continue to Windshift'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
