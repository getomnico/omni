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

    let { open = false, onSuccess, onCancel }: Props = $props()

    let domain = $state('')
    let saToken = $state('')
    let orgId = $state('')
    let orgAdminApiKey = $state('')
    let isSubmitting = $state(false)

    function reset() {
        domain = ''
        saToken = ''
        orgId = ''
        orgAdminApiKey = ''
    }

    function normalizedDomain(): string {
        // Strip scheme + trailing slash so we always store the bare host.
        let d = domain.trim()
        d = d.replace(/^https?:\/\//, '').replace(/\/+$/, '')
        return d
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            if (!domain.trim()) {
                throw new Error('Atlassian domain is required')
            }
            if (!saToken.trim()) {
                throw new Error('Service account token is required')
            }
            if ((orgId.trim() && !orgAdminApiKey.trim()) || (!orgId.trim() && orgAdminApiKey.trim())) {
                throw new Error('Organization ID and Organization API Key must be provided together')
            }

            const credentials: Record<string, string> = {
                sa_token: saToken.trim(),
            }
            const credentialConfig: Record<string, string> = {
                domain: normalizedDomain(),
            }
            if (orgId.trim() && orgAdminApiKey.trim()) {
                credentials.org_admin_api_key = orgAdminApiKey.trim()
                credentialConfig.org_id = orgId.trim()
            }

            const authType = AuthType.API_KEY
            const provider = 'atlassian'

            // Create Confluence source (per-source config holds only filter prefs).
            const confluenceConfig: ConfluenceSourceConfig = {}
            const confluenceSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Confluence',
                    sourceType: 'confluence',
                    config: confluenceConfig,
                }),
            })

            if (!confluenceSourceResponse.ok) {
                throw new Error('Failed to create Confluence source')
            }

            const confluenceSource = await confluenceSourceResponse.json()

            const confluenceCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: confluenceSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: null,
                    credentials: credentials,
                    config: credentialConfig,
                }),
            })

            if (!confluenceCredentialsResponse.ok) {
                throw new Error('Failed to create Confluence service credentials')
            }

            const jiraConfig: JiraSourceConfig = {}
            const jiraSourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'JIRA',
                    sourceType: 'jira',
                    config: jiraConfig,
                }),
            })

            if (!jiraSourceResponse.ok) {
                throw new Error('Failed to create JIRA source')
            }

            const jiraSource = await jiraSourceResponse.json()

            const jiraCredentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: jiraSource.id,
                    provider: provider,
                    authType: authType,
                    principalEmail: null,
                    credentials: credentials,
                    config: credentialConfig,
                }),
            })

            if (!jiraCredentialsResponse.ok) {
                throw new Error('Failed to create JIRA service credentials')
            }

            toast.success('Atlassian connected successfully!')
            reset()

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
        reset()
        if (onCancel) {
            onCancel()
        }
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Atlassian</Dialog.Title>
            <Dialog.Description>
                Connect Confluence and Jira using an Atlassian service account.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            <div class="space-y-2">
                <Label for="domain">Atlassian Domain</Label>
                <Input
                    id="domain"
                    bind:value={domain}
                    placeholder="company.atlassian.net"
                    type="text"
                    required />
                <p class="text-muted-foreground text-sm">
                    Your Atlassian site (e.g., company.atlassian.net).
                </p>
            </div>

            <div class="space-y-2">
                <Label for="sa-token">Service Account Token</Label>
                <Input
                    id="sa-token"
                    bind:value={saToken}
                    placeholder="ATSTT…"
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Create a service account at <a
                        href="https://admin.atlassian.com/"
                        target="_blank"
                        class="text-blue-600 hover:underline">admin.atlassian.com</a> →
                    Service accounts. Free orgs include 5. Grant the service account
                    Confluence and Jira product access, and issue a token with read
                    scopes for both products.
                </p>
            </div>

            <div class="border-t pt-4 space-y-4">
                <div>
                    <h3 class="text-sm font-medium">Organization Admin (optional)</h3>
                    <p class="text-muted-foreground text-xs">
                        Improves coverage for users with private email visibility.
                        Requires Atlassian Guard.
                    </p>
                </div>

                <div class="space-y-2">
                    <Label for="org-id">Organization ID</Label>
                    <Input
                        id="org-id"
                        bind:value={orgId}
                        placeholder="UUID from admin.atlassian.com"
                        type="text" />
                </div>

                <div class="space-y-2">
                    <Label for="org-admin-api-key">Organization API Key</Label>
                    <Input
                        id="org-admin-api-key"
                        bind:value={orgAdminApiKey}
                        placeholder="Bearer token from admin.atlassian.com → API keys"
                        type="password" />
                </div>
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
