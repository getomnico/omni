<script lang="ts">
    import * as Dialog from '$lib/components/ui/dialog'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { toast } from 'svelte-sonner'

    interface Props {
        open: boolean
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { open = false, onSuccess, onCancel }: Props = $props()
    let accountUrl = $state('')
    let warehouse = $state('')
    let role = $state('')
    let databases = $state('')
    let privateKey = $state('')
    let username = $state('')
    let privateKeyPassphrase = $state('')
    let mcpEndpoint = $state('')
    let mcpClientId = $state('')
    let mcpClientSecret = $state('')
    let mcpEnabled = $state(false)
    let syncEnabled = $state(true)
    let submitting = $state(false)

    function httpsOrigin(value: string, label: string): string {
        const parsed = new URL(value.trim())
        if (
            parsed.protocol !== 'https:' ||
            parsed.username ||
            parsed.password ||
            parsed.port ||
            parsed.search ||
            parsed.hash ||
            parsed.pathname !== '/'
        ) {
            throw new Error(`${label} must be a credential-free HTTPS account URL`)
        }
        return parsed.origin
    }

    async function submit() {
        submitting = true
        try {
            const normalizedAccount = httpsOrigin(accountUrl, 'Snowflake account URL')
            if (!username.trim() || !privateKey.trim() || !warehouse.trim() || !role.trim()) {
                throw new Error('Metadata username, private key, warehouse, and role are required')
            }
            if (mcpEnabled && (!mcpEndpoint.trim() || !mcpClientId.trim() || !mcpClientSecret.trim())) {
                throw new Error('MCP endpoint, OAuth client ID, and OAuth client secret are required')
            }
            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    scope: 'org',
                    name: 'Snowflake',
                    sourceType: 'snowflake',
                    config: {
                        account_url: normalizedAccount,
                        warehouse: warehouse.trim(),
                        role: role.trim(),
                        databases: databases.split(',').map((value) => value.trim()).filter(Boolean),
                        sync_enabled: syncEnabled,
                        mcp_enabled: mcpEnabled,
                        mcp_endpoint_url: mcpEnabled ? mcpEndpoint.trim() : null,
                    },
                }),
            })
            if (!sourceResponse.ok) throw new Error('Failed to create Snowflake source')
            const source = await sourceResponse.json()
            const credentialResponse = await fetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: source.id,
                    provider: 'snowflake',
                    authType: 'jwt',
                    credentials: {
                        username: username.trim(),
                        private_key: privateKey,
                        private_key_passphrase: privateKeyPassphrase || null,
                        mcp_oauth_client_id: mcpEnabled ? mcpClientId.trim() : null,
                        mcp_oauth_client_secret: mcpEnabled ? mcpClientSecret : null,
                    },
                }),
            })
            if (!credentialResponse.ok) throw new Error('Failed to save Snowflake metadata credentials')
            if (mcpEnabled) {
                toast.success('Snowflake configured. Starting OAuth discovery.')
                window.location.href = `/api/oauth/start?source_id=${encodeURIComponent(source.id)}&flow=user_read`
                return
            }
            toast.success('Snowflake connected successfully')
            onSuccess?.()
        } catch (error: unknown) {
            toast.error(error instanceof Error ? error.message : 'Failed to set up Snowflake')
        } finally {
            submitting = false
        }
    }
</script>

<Dialog.Root bind:open>
    <Dialog.Content class="max-w-2xl">
        <Dialog.Header>
            <Dialog.Title>Connect Snowflake</Dialog.Title>
            <Dialog.Description>Index metadata only. Warehouse rows and query results remain live-only.</Dialog.Description>
        </Dialog.Header>
        <div class="grid gap-3">
            <Label for="snowflake-account">Account URL</Label>
            <Input id="snowflake-account" bind:value={accountUrl} placeholder="https://account.snowflakecomputing.com" />
            <Label for="snowflake-databases">Databases (comma separated)</Label>
            <Input id="snowflake-databases" bind:value={databases} placeholder="ANALYTICS" />
            <Label for="snowflake-warehouse">Metadata warehouse</Label>
            <Input id="snowflake-warehouse" bind:value={warehouse} />
            <Label for="snowflake-role">Metadata role</Label>
            <Input id="snowflake-role" bind:value={role} />
            <Label for="snowflake-user">Metadata service username</Label>
            <Input id="snowflake-user" bind:value={username} />
            <Label for="snowflake-key">Private key (PEM)</Label>
            <textarea id="snowflake-key" class="min-h-24 rounded-md border p-2 text-sm" bind:value={privateKey}></textarea>
            <Label for="snowflake-passphrase">Private key passphrase (optional)</Label>
            <Input id="snowflake-passphrase" type="password" bind:value={privateKeyPassphrase} />
            <label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={syncEnabled} /> Sync metadata</label>
            <label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={mcpEnabled} /> Enable managed MCP tools</label>
            {#if mcpEnabled}
                <Label for="snowflake-mcp">Managed MCP endpoint</Label>
                <Input id="snowflake-mcp" bind:value={mcpEndpoint} placeholder="https://account/api/v2/databases/.../mcp-servers/..." />
                <Label for="snowflake-client-id">OAuth client ID</Label>
                <Input id="snowflake-client-id" bind:value={mcpClientId} />
                <Label for="snowflake-client-secret">OAuth client secret</Label>
                <Input id="snowflake-client-secret" type="password" bind:value={mcpClientSecret} />
                <p class="rounded-md bg-amber-50 p-3 text-sm text-amber-900">Snowflake MCP tools have not been discovered. Connect Snowflake and discover tools using a user OAuth credential.</p>
                <Button class="cursor-pointer" variant="outline" onclick={submit}>Connect Snowflake and discover tools</Button>
            {/if}
        </div>
        <Dialog.Footer>
            <Button class="cursor-pointer" variant="outline" onclick={() => onCancel?.()}>Cancel</Button>
            <Button class="cursor-pointer" disabled={submitting} onclick={submit}>Connect Snowflake</Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
