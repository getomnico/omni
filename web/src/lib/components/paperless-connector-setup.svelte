<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType, ServiceProvider, SourceType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()

    let sourceName = $state('Paperless-ngx')
    let baseUrl = $state('')
    let apiKey = $state('')

    let isSubmitting = $state(false)

    async function handleSubmit() {
        if (!baseUrl.trim()) {
            toast.error('Paperless-ngx URL is required')
            return
        }
        if (!apiKey.trim()) {
            toast.error('API key is required')
            return
        }

        isSubmitting = true

        try {
            const config = {
                base_url: baseUrl.trim().replace(/\/$/, ''),
                sync_enabled: true,
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: sourceName.trim() || 'Paperless-ngx',
                    sourceType: SourceType.PAPERLESS_NGX,
                    config,
                }),
            })

            if (!sourceResponse.ok) {
                const text = await sourceResponse.text()
                throw new Error(`Failed to create Paperless-ngx source: ${text}`)
            }

            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: ServiceProvider.PAPERLESS_NGX,
                    authType: AuthType.API_KEY,
                    credentials: { api_key: apiKey.trim() },
                }),
            })

            if (!credentialsResponse.ok) {
                const text = await credentialsResponse.text()
                throw new Error(`Failed to save API key: ${text}`)
            }

            toast.success('Paperless-ngx connected successfully!')
            resetForm()

            if (onSuccess) {
                onSuccess()
            }
        } catch (err: any) {
            console.error('Error setting up Paperless-ngx:', err)
            toast.error(err.message || 'Failed to connect Paperless-ngx')
        } finally {
            isSubmitting = false
        }
    }

    function resetForm() {
        sourceName = 'Paperless-ngx'
        baseUrl = ''
        apiKey = ''
    }

    function handleCancel() {
        resetForm()
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-lg">
        <Dialog.Header>
            <Dialog.Title>Connect Paperless-ngx</Dialog.Title>
            <Dialog.Description>
                Index documents and their OCR content from your paperless-ngx instance. Credentials
                are stored encrypted and never leave the server.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-1.5">
                <Label for="paperless-name">Source name</Label>
                <Input
                    id="paperless-name"
                    bind:value={sourceName}
                    placeholder="e.g. Home Documents"
                    disabled={isSubmitting} />
            </div>

            <div class="space-y-1.5">
                <Label for="paperless-url">Paperless-ngx URL</Label>
                <Input
                    id="paperless-url"
                    bind:value={baseUrl}
                    placeholder="https://paperless.example.com"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    The base URL of your paperless-ngx instance (without trailing slash).
                </p>
            </div>

            <div class="space-y-1.5">
                <Label for="paperless-key">API key</Label>
                <Input
                    id="paperless-key"
                    type="password"
                    bind:value={apiKey}
                    placeholder="Your paperless-ngx API token"
                    autocomplete="current-password"
                    disabled={isSubmitting}
                    required />
                <p class="text-muted-foreground text-xs">
                    Generate an API token in paperless-ngx under <em>Settings → API</em>.
                </p>
            </div>
        </div>

        <Dialog.Footer>
            <Button
                variant="outline"
                onclick={handleCancel}
                disabled={isSubmitting}
                class="cursor-pointer">
                Cancel
            </Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting…' : 'Connect'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
