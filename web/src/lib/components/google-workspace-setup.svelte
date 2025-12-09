<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import { goto } from '$app/navigation'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

    let serviceAccountJson = $state('')
    let principalEmail = $state('')
    let domain = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!serviceAccountJson.trim()) {
                throw new Error('Service account JSON is required')
            }

            if (!principalEmail.trim()) {
                throw new Error('Admin email is required')
            }

            if (!domain.trim()) {
                throw new Error('Organization domain is required')
            }

            // Validate JSON
            try {
                JSON.parse(serviceAccountJson)
            } catch {
                throw new Error('Invalid JSON format')
            }

            const credentials = { service_account_key: serviceAccountJson }
            const config = {
                domain: domain || null,
            }
            const authType = AuthType.JWT
            const provider = 'google'

            // Create Google Drive source
            const driveSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Google Drive',
                    sourceType: 'google_drive',
                    config,
                }),
            })

            if (!driveSourceResponse.ok) {
                throw new Error('Failed to create Google Drive source')
            }

            const driveSource = await driveSourceResponse.json()

            // Create service credentials for Drive source
            const driveCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: driveSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials,
                    config,
                }),
            })

            if (!driveCredentialsResponse.ok) {
                throw new Error('Failed to create Google Drive service credentials')
            }

            // Create Gmail source
            const gmailSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Gmail',
                    sourceType: 'gmail',
                    config,
                }),
            })

            if (!gmailSourceResponse.ok) {
                throw new Error('Failed to create Gmail source')
            }

            const gmailSource = await gmailSourceResponse.json()

            // Create service credentials for Gmail source (same credentials as Drive)
            const gmailCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: gmailSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials: credentials,
                    config,
                }),
            })

            if (!gmailCredentialsResponse.ok) {
                throw new Error('Failed to create Gmail service credentials')
            }

            toast.success('Google Workspace connected successfully!')
            open = false

            // Reset form
            serviceAccountJson = ''
            principalEmail = ''
            domain = ''

            // Call success callback if provided
            if (onSuccess) {
                onSuccess()
            } else {
                // Default behavior: redirect to configure page
                await goto('/admin/settings/integrations/google')
            }
        } catch (error: any) {
            console.error('Error setting up Google Workspace:', error)
            toast.error(error.message || 'Failed to set up Google Workspace')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        serviceAccountJson = ''
        principalEmail = ''
        domain = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Google Workspace</Dialog.Title>
            <Dialog.Description>
                Set up your Google Workspace integration using service account credentials.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="service-account-json">Service Account JSON Key</Label>
                <Textarea
                    id="service-account-json"
                    bind:value={serviceAccountJson}
                    placeholder="Paste your Google service account JSON key here..."
                    rows={10}
                    class="max-h-64 overflow-y-auto font-mono text-sm break-all whitespace-pre-wrap" />
                <p class="text-muted-foreground text-sm">
                    Download this from the Google Cloud Console under "Service Accounts" > "Keys".
                </p>
            </div>

            <div class="space-y-2">
                <Label for="principal-email">Admin Email</Label>
                <Input
                    id="principal-email"
                    bind:value={principalEmail}
                    placeholder="admin@yourdomain.com"
                    type="email"
                    required />
                <p class="text-muted-foreground text-sm">
                    The admin user email that the service account will impersonate to access Google
                    Workspace APIs.
                </p>
            </div>

            <div class="space-y-2">
                <Label for="domain">Organization Domain</Label>
                <Input
                    id="domain"
                    bind:value={domain}
                    placeholder="yourdomain.com"
                    type="text"
                    required />
                <p class="text-muted-foreground text-sm">
                    Your Google Workspace domain (e.g., company.com). The service account will
                    impersonate all users in this domain.
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
