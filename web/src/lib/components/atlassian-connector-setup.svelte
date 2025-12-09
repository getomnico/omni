<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType, type ConfluenceSourceConfig, type JiraSourceConfig } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = $bindable(false), onSuccess, onCancel }: Props = $props()

    let principalEmail = $state('')
    let apiToken = $state('')
    let domain = $state('')
    let isSubmitting = $state(false)

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!principalEmail.trim()) {
                throw new Error('Email is required')
            }

            if (!apiToken.trim()) {
                throw new Error('API token is required')
            }

            if (!domain.trim()) {
                throw new Error('Atlassian domain is required')
            }

            const credentials = {
                api_token: apiToken,
            }
            const baseUrl = domain.startsWith('http') ? domain : `https://${domain}`
            const authType = AuthType.API_KEY
            const provider = 'atlassian'

            // Create Confluence source
            const confluenceConfig: ConfluenceSourceConfig = {
                base_url: baseUrl,
            }
            const confluenceSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'Confluence',
                    sourceType: 'confluence',
                    config: confluenceConfig,
                }),
            })

            if (!confluenceSourceResponse.ok) {
                throw new Error('Failed to create Confluence source')
            }

            const confluenceSource = await confluenceSourceResponse.json()

            // Create service credentials for Confluence source
            const confluenceCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: confluenceSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials: credentials,
                    config: confluenceConfig,
                }),
            })

            if (!confluenceCredentialsResponse.ok) {
                throw new Error('Failed to create Confluence service credentials')
            }

            // Create JIRA source
            const jiraConfig: JiraSourceConfig = {
                base_url: baseUrl,
            }
            const jiraSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: 'JIRA',
                    sourceType: 'jira',
                    config: jiraConfig,
                }),
            })

            if (!jiraSourceResponse.ok) {
                throw new Error('Failed to create JIRA source')
            }

            const jiraSource = await jiraSourceResponse.json()

            // Create service credentials for JIRA source (same credentials as Confluence)
            const jiraCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: jiraSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: principalEmail || null,
                    credentials: credentials,
                    config: jiraConfig,
                }),
            })

            if (!jiraCredentialsResponse.ok) {
                throw new Error('Failed to create JIRA service credentials')
            }

            toast.success('Atlassian connected successfully!')
            open = false

            // Reset form
            principalEmail = ''
            apiToken = ''
            domain = ''

            // Call success callback if provided
            if (onSuccess) {
                onSuccess()
            }
        } catch (error: any) {
            console.error('Error setting up Atlassian:', error)
            toast.error(error.message || 'Failed to set up Atlassian')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        open = false
        principalEmail = ''
        apiToken = ''
        domain = ''
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Atlassian</Dialog.Title>
            <Dialog.Description>
                Set up your Atlassian integration using an API token.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="principal-email">Your Atlassian Email</Label>
                <Input
                    id="principal-email"
                    bind:value={principalEmail}
                    placeholder="your.email@company.com"
                    type="email"
                    required />
            </div>

            <div class="space-y-2">
                <Label for="api-token">API Token</Label>
                <Input
                    id="api-token"
                    bind:value={apiToken}
                    placeholder="Your Atlassian API token"
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create an API token at <a
                        href="https://id.atlassian.com/manage-profile/security/api-tokens"
                        target="_blank"
                        class="text-blue-600 hover:underline">id.atlassian.com</a>
                </p>
            </div>

            <div class="space-y-2">
                <Label for="domain">Atlassian Domain</Label>
                <Input
                    id="domain"
                    bind:value={domain}
                    placeholder="company.atlassian.net"
                    type="text"
                    required />
                <p class="text-muted-foreground text-sm">
                    Your Atlassian site domain (e.g., company.atlassian.net)
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
