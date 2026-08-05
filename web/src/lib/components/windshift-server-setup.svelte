<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        baseUrl?: string
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, baseUrl = '', onSuccess, onCancel }: Props = $props()

    let isSubmitting = $state(false)

    function normalizeBaseUrl(value: string): string {
        return value.trim().replace(/\/+$/, '')
    }

    function validateBaseUrl(value: string): string | null {
        let url: URL
        try {
            url = new URL(value)
        } catch {
            return 'Windshift URL must be a valid URL'
        }
        if (url.protocol !== 'https:' && url.protocol !== 'http:') {
            return 'Windshift URL must use http or https'
        }
        if (url.username || url.password) {
            return 'Windshift URL must not include credentials'
        }
        if (url.hash) {
            return 'Windshift URL must not include a fragment'
        }
        return null
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            const trimmedBaseUrl = normalizeBaseUrl(baseUrl)
            if (!trimmedBaseUrl) {
                throw new Error('Windshift URL is required')
            }
            const baseUrlError = validateBaseUrl(trimmedBaseUrl)
            if (baseUrlError) {
                throw new Error(baseUrlError)
            }

            const response = await fetch('/api/connector-configs', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    provider: 'windshift',
                    config: {
                        base_url: trimmedBaseUrl,
                    },
                }),
            })

            if (!response.ok) {
                let message = 'Failed to save Windshift server configuration'
                try {
                    const body = await response.json()
                    if (typeof body?.message === 'string' && body.message) {
                        message = body.message
                    }
                } catch {
                    // Non-JSON error body; keep the generic message
                }
                throw new Error(message)
            }

            toast.success('Windshift server configuration saved')

            baseUrl = ''

            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error saving Windshift server configuration:', error)
            toast.error(error.message || 'Failed to save Windshift server configuration')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        baseUrl = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Windshift server</Dialog.Title>
            <Dialog.Description>
                Point Omni at your Windshift instance. Users can then connect Windshift from My
                Integrations; the connector uses these URLs for OAuth, sync, and MCP actions.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="windshift-base-url">Windshift URL</Label>
                <Input
                    id="windshift-base-url"
                    bind:value={baseUrl}
                    placeholder="https://windshift.example.com"
                    required />
                <p class="text-muted-foreground text-sm">
                    The URL of your Windshift instance, reachable from the user's browser (OAuth
                    consent, resource binding, document links). Include a context path if Windshift
                    is served under one.
                </p>
            </div>

            <p class="text-muted-foreground text-xs">
                The URL is validated against SSRF: only publicly routable http(s) addresses are
                accepted; loopback and private-network addresses are rejected. To route
                server-to-server traffic (token exchange, sync, MCP) over a private network instead
                of through this URL, set the WINDSHIFT_INTERNAL_BASE_URL environment variable on the
                connector container.
            </p>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Saving...' : 'Save'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
