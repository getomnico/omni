<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { AuthType, ServiceProvider, SourceType, type GoogleAdsSourceConfig } from '$lib/types'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        oauthConfigured?: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, oauthConfigured = false, onCancel }: Props = $props()

    let sourceName = $state('Google Ads')
    let developerToken = $state('')
    let customerIds = $state('')
    let loginCustomerId = $state('')
    let isSubmitting = $state(false)

    function reset() {
        sourceName = 'Google Ads'
        developerToken = ''
        customerIds = ''
        loginCustomerId = ''
    }

    function parseCustomerIds(): string[] {
        return customerIds
            .split(/[\s,]+/)
            .map((id) => id.replace(/-/g, '').trim())
            .filter(Boolean)
    }

    async function handleSubmit() {
        isSubmitting = true
        try {
            const ids = parseCustomerIds()
            if (!oauthConfigured) {
                throw new Error('Configure the Google Ads OAuth client before connecting')
            }
            if (!developerToken.trim()) throw new Error('Developer token is required')
            if (ids.length === 0) throw new Error('At least one customer ID is required')

            const config: GoogleAdsSourceConfig = {
                customer_ids: ids,
                ...(loginCustomerId.trim()
                    ? { login_customer_id: loginCustomerId.replace(/-/g, '').trim() }
                    : {}),
                sync_enabled: true,
            }

            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: sourceName.trim() || 'Google Ads',
                    sourceType: SourceType.GOOGLE_ADS,
                    config,
                }),
            })
            if (!sourceResponse.ok) throw new Error('Failed to create Google Ads source')
            const source = await sourceResponse.json()

            const credentialsResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: ServiceProvider.GOOGLE_ADS,
                    authType: AuthType.OAUTH,
                    credentials: {
                        developer_token: developerToken.trim(),
                    },
                    config: {},
                    triggerSync: false,
                }),
            })
            if (!credentialsResponse.ok) throw new Error('Failed to create Google Ads credentials')

            toast.success('Google Ads source created. Continue with Google to authorize access.')
            window.location.href = `/api/oauth/start?org_source_id=${source.id}&return_to=${encodeURIComponent('/admin/settings/integrations?success=connected')}`
        } catch (error: any) {
            console.error('Error setting up Google Ads:', error)
            toast.error(error.message || 'Failed to set up Google Ads')
        } finally {
            isSubmitting = false
        }
    }

    function handleCancel() {
        reset()
        onCancel?.()
    }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && handleCancel()}>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Google Ads</Dialog.Title>
            <Dialog.Description>
                Index Google Ads account structure and enable live GAQL/report actions. Numeric
                performance metrics are fetched live by actions, not synced into the index.
            </Dialog.Description>
        </Dialog.Header>

        <div class="space-y-4">
            {#if !oauthConfigured}
                <div
                    class="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900">
                    Configure a Google Ads OAuth client in the OAuth app clients tab before
                    connecting. Omni will use that OAuth client to authorize access with Google.
                </div>
            {/if}

            <div class="space-y-2">
                <Label for="google-ads-name">Source name</Label>
                <Input id="google-ads-name" bind:value={sourceName} placeholder="Google Ads" />
            </div>

            <div class="space-y-2">
                <Label for="google-ads-developer-token">Developer token</Label>
                <Input
                    id="google-ads-developer-token"
                    bind:value={developerToken}
                    type="password"
                    required />
                <p class="text-muted-foreground text-sm">
                    Required by the Google Ads API. Configure production/basic access in Google Ads
                    API Center.
                </p>
            </div>

            <div class="space-y-2">
                <Label for="google-ads-customer-ids">Customer IDs</Label>
                <Input
                    id="google-ads-customer-ids"
                    bind:value={customerIds}
                    placeholder="1234567890, 0987654321"
                    required />
                <p class="text-muted-foreground text-sm">
                    Comma- or space-separated Google Ads customer IDs to index.
                </p>
            </div>

            <div class="space-y-2">
                <Label for="google-ads-login-customer-id"
                    >Manager/login customer ID (optional)</Label>
                <Input
                    id="google-ads-login-customer-id"
                    bind:value={loginCustomerId}
                    placeholder="1234567890" />
            </div>
        </div>

        <Dialog.Footer>
            <Button variant="outline" onclick={handleCancel} class="cursor-pointer">Cancel</Button>
            <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
                {isSubmitting ? 'Connecting...' : 'Continue with Google'}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
