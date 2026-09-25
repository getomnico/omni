<script lang="ts">
    import { enhance } from '$app/forms'
    import { beforeNavigate } from '$app/navigation'
    import { onMount, untrack } from 'svelte'
    import { toast } from 'svelte-sonner'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Badge } from '$lib/components/ui/badge'
    import { KeyRound, Loader2, AlertTriangle } from '@lucide/svelte'
    import type { PageProps } from './$types'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'

    let { data }: PageProps = $props()
    let enabled = $state(untrack(() => data.source.isActive))
    let originalEnabled = untrack(() => data.source.isActive)
    let syncEnabled = $state(untrack(() => data.config.syncEnabled))
    let mcpEnabled = $state(untrack(() => data.config.mcpEnabled))
    let includeTags = $state(untrack(() => data.config.includeTags))
    let writeToolsEnabled = $state(untrack(() => data.config.writeToolsEnabled))
    let readOnly = $state(untrack(() => data.config.readOnly))
    let submitting = $state(false)
    let skipUnsavedCheck = $state(false)
    let accountUrl = $state(untrack(() => data.config.accountUrl))
    let warehouse = $state(untrack(() => data.config.warehouse))
    let role = $state(untrack(() => data.config.role))
    let databases = $state(untrack(() => data.config.databases.join(', ')))
    let schemasAllowlist = $state(untrack(() => data.config.schemasAllowlist.join(', ')))
    let schemasDenylist = $state(untrack(() => data.config.schemasDenylist.join(', ')))
    let includedObjectTypes = $state(untrack(() => data.config.includedObjectTypes.join(', ')))
    let mcpEndpointUrl = $state(untrack(() => data.config.mcpEndpointUrl))
    let hasUnsavedChanges = $derived(
        enabled !== originalEnabled ||
            syncEnabled !== data.config.syncEnabled ||
            mcpEnabled !== data.config.mcpEnabled ||
            includeTags !== data.config.includeTags ||
            writeToolsEnabled !== data.config.writeToolsEnabled ||
            readOnly !== data.config.readOnly ||
            accountUrl !== data.config.accountUrl ||
            warehouse !== data.config.warehouse ||
            role !== data.config.role ||
            databases !== data.config.databases.join(', ') ||
            schemasAllowlist !== data.config.schemasAllowlist.join(', ') ||
            schemasDenylist !== data.config.schemasDenylist.join(', ') ||
            includedObjectTypes !== data.config.includedObjectTypes.join(', ') ||
            mcpEndpointUrl !== data.config.mcpEndpointUrl,
    )

    let oauthDialogOpen = $state(false)
    let oauthDialogLoading = $state(false)
    let oauthConfigured = $state<boolean | null>(null)
    let oauthProvider = $state<{
        provider: string
        displayName: string
        configured: boolean
        updatedAt: string | null
        config: Record<string, unknown>
    } | null>(null)

    async function loadOAuthConfig() {
        const response = await fetch('/api/connector-configs')
        if (!response.ok) throw new Error('Failed to load Snowflake OAuth configuration')
        const configs = (await response.json()) as Array<{
            provider: string
            config: Record<string, unknown>
            updatedAt: string
        }>
        const provider = `snowflake:${data.source.id}`
        const saved = configs.find((item) => item.provider === provider)
        const config = saved?.config ?? {}
        const configured =
            typeof config.oauth_client_id === 'string' &&
            config.oauth_client_id.length > 0 &&
            ((typeof config.oauth_client_secret === 'string' &&
                config.oauth_client_secret.length > 0) ||
                config.oauth_dynamic_client_registration === 'true')
        return {
            provider,
            displayName: `Snowflake — ${data.source.name} MCP OAuth`,
            configured,
            updatedAt: saved?.updatedAt ?? null,
            config,
        }
    }
    async function checkOAuth() {
        try {
            oauthConfigured = (await loadOAuthConfig()).configured
        } catch {
            oauthConfigured = null
        }
    }
    async function openOAuthDialog() {
        oauthDialogLoading = true
        try {
            oauthProvider = await loadOAuthConfig()
            oauthConfigured = oauthProvider.configured
            oauthDialogOpen = true
        } catch (err) {
            toast.error(err instanceof Error ? err.message : 'Failed to load OAuth configuration')
        } finally {
            oauthDialogLoading = false
        }
    }

    let beforeUnloadHandler: ((event: BeforeUnloadEvent) => void) | null = null
    onMount(() => {
        beforeUnloadHandler = (event) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                event.preventDefault()
                event.returnValue = ''
            }
        }
        window.addEventListener('beforeunload', beforeUnloadHandler)
        void checkOAuth()
        return () => {
            if (beforeUnloadHandler) window.removeEventListener('beforeunload', beforeUnloadHandler)
        }
    })
    beforeNavigate(({ cancel }) => {
        if (
            hasUnsavedChanges &&
            !skipUnsavedCheck &&
            !confirm('You have unsaved changes. Are you sure you want to leave this page?')
        )
            cancel()
    })
</script>

<svelte:head><title>Configure Snowflake - {data.source.name}</title></svelte:head>
<form
    method="POST"
    use:enhance={() => {
        submitting = true
        return async ({ result, update }) => {
            if (result.type === 'failure') {
                const message = result.data?.message
                toast.error(
                    typeof message === 'string' ? message : 'Failed to save Snowflake settings',
                )
            }
            if (result.type === 'redirect') {
                skipUnsavedCheck = true
                if (beforeUnloadHandler)
                    window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
            await update()
            submitting = false
        }
    }}>
    <Card.Root>
        <Card.Header>
            <div class="flex items-start justify-between gap-4">
                <div>
                    <Card.Title>{data.source.name}</Card.Title><Card.Description class="mt-1"
                        >Manage Snowflake metadata indexing and live-only MCP tools independently.</Card.Description>
                </div>
                <div class="flex items-center gap-2">
                    <Label for="enabled">Enabled</Label><Switch
                        id="enabled"
                        bind:checked={enabled}
                        name="enabled"
                        class="cursor-pointer" />
                </div>
            </div>
        </Card.Header>
        <Card.Content class="space-y-6">
            <section class="space-y-3">
                <h2 class="font-semibold">Metadata indexing</h2>
                <p class="text-muted-foreground text-sm">
                    Only metadata is indexed; warehouse rows and query results remain live-only.
                </p>
                <div>
                    <Label for="accountUrl">Snowflake account URL</Label><Input
                        id="accountUrl"
                        name="accountUrl"
                        bind:value={accountUrl}
                        required />
                </div>
                <label class="flex items-center gap-2 text-sm"
                    ><input
                        type="checkbox"
                        name="syncEnabled"
                        value="true"
                        bind:checked={syncEnabled} /> Sync metadata</label>
                <input type="hidden" name="syncEnabled" value="false" />
                {#if syncEnabled}
                    <div class="grid gap-3 md:grid-cols-2">
                        <div>
                            <Label for="warehouse">Metadata warehouse</Label><Input
                                id="warehouse"
                                name="warehouse"
                                bind:value={warehouse}
                                required />
                        </div>
                        <div>
                            <Label for="role">Metadata role</Label><Input
                                id="role"
                                name="role"
                                bind:value={role}
                                required />
                        </div>
                        <div>
                            <Label for="databases">Databases (comma separated)</Label><Input
                                id="databases"
                                name="databases"
                                bind:value={databases}
                                required />
                        </div>
                        <div>
                            <Label for="schemasAllowlist">Schema allowlist (optional)</Label><Input
                                id="schemasAllowlist"
                                name="schemasAllowlist"
                                bind:value={schemasAllowlist} />
                        </div>
                        <div>
                            <Label for="schemasDenylist">Schema denylist (optional)</Label><Input
                                id="schemasDenylist"
                                name="schemasDenylist"
                                bind:value={schemasDenylist} />
                        </div>
                        <div>
                            <Label for="includedObjectTypes">Included object types</Label><Input
                                id="includedObjectTypes"
                                name="includedObjectTypes"
                                bind:value={includedObjectTypes} />
                        </div>
                    </div>
                    <label class="flex items-center gap-2 text-sm"
                        ><input
                            type="checkbox"
                            name="includeTags"
                            value="true"
                            bind:checked={includeTags} /> Include tags in metadata</label>
                {:else}
                    <p class="text-muted-foreground text-sm">
                        Metadata sync is disabled. MCP actions can still use live Snowflake access.
                    </p>
                    <input type="hidden" name="warehouse" value={warehouse} />
                    <input type="hidden" name="role" value={role} />
                    <input type="hidden" name="databases" value={databases} />
                    <input type="hidden" name="schemasAllowlist" value={schemasAllowlist} />
                    <input type="hidden" name="schemasDenylist" value={schemasDenylist} />
                    <input type="hidden" name="includedObjectTypes" value={includedObjectTypes} />
                    <input
                        type="hidden"
                        name="includeTags"
                        value={includeTags ? 'true' : 'false'} />
                {/if}
            </section>
            <section class="space-y-3 border-t pt-5">
                <h2 class="font-semibold">Managed MCP actions</h2>
                <p class="text-muted-foreground text-sm">
                    Live actions use per-user Snowflake OAuth and do not sync or index query
                    results.
                </p>
                <label class="flex items-center gap-2 text-sm"
                    ><input
                        type="checkbox"
                        name="mcpEnabled"
                        value="true"
                        bind:checked={mcpEnabled} /> Enable managed MCP tools</label>
                <input type="hidden" name="mcpEnabled" value="false" />
                {#if mcpEnabled}
                    <Label for="mcpEndpointUrl">Managed MCP endpoint</Label><Input
                        id="mcpEndpointUrl"
                        name="mcpEndpointUrl"
                        bind:value={mcpEndpointUrl}
                        required />
                    <label class="flex items-center gap-2 text-sm"
                        ><input
                            type="checkbox"
                            name="writeToolsEnabled"
                            value="true"
                            bind:checked={writeToolsEnabled} /> Permit write-capable tools</label>
                    <input type="hidden" name="writeToolsEnabled" value="false" />
                    <label class="flex items-center gap-2 text-sm"
                        ><input
                            type="checkbox"
                            name="readOnly"
                            value="true"
                            bind:checked={readOnly} /> Keep source read-only</label>
                    <input type="hidden" name="readOnly" value="false" />
                {:else}
                    <input type="hidden" name="mcpEndpointUrl" value={mcpEndpointUrl} />
                    <input
                        type="hidden"
                        name="writeToolsEnabled"
                        value={writeToolsEnabled ? 'true' : 'false'} />
                    <input type="hidden" name="readOnly" value={readOnly ? 'true' : 'false'} />
                {/if}
            </section>
        </Card.Content>
        <Card.Footer class="justify-end"
            ><Button
                type="submit"
                disabled={submitting || !hasUnsavedChanges}
                class="cursor-pointer"
                >{#if submitting}<Loader2 class="mr-2 h-4 w-4 animate-spin" />{/if}Save settings</Button
            ></Card.Footer>
    </Card.Root>
</form>

<Card.Root class="mt-5">
    <Card.Header
        ><Card.Title class="flex items-center gap-2"
            ><KeyRound class="h-5 w-5" />MCP user OAuth</Card.Title
        ><Card.Description
            >OAuth client configuration is scoped to this Snowflake source. Client secrets are
            write-only and are never shown here.</Card.Description
        ></Card.Header>
    <Card.Content>
        {#if data.actionAuth.authorized}<Badge variant="secondary"
                >Authorized{data.actionAuth.principalEmail
                    ? ` as ${data.actionAuth.principalEmail}`
                    : ''}</Badge
            >{:else}<Badge variant="outline">Not authorized for your account</Badge>{/if}
        {#if data.actionAuth.grantedScopes.length}<p class="text-muted-foreground mt-2 text-sm">
                Scopes: {data.actionAuth.grantedScopes.join(', ')}
            </p>{/if}
        {#if oauthConfigured === false}<Alert.Root class="mt-3"
                ><AlertTriangle class="h-4 w-4" /><Alert.Title>OAuth client required</Alert.Title
                ><Alert.Description
                    >Configure a source-scoped OAuth client before authorizing Snowflake.</Alert.Description
                ></Alert.Root
            >{/if}
    </Card.Content>
    <Card.Footer class="justify-end gap-2">
        <Button
            type="button"
            variant="outline"
            disabled={oauthDialogLoading}
            class="cursor-pointer"
            onclick={openOAuthDialog}
            >{#if oauthDialogLoading}<Loader2
                    class="mr-2 h-4 w-4 animate-spin" />{/if}{oauthConfigured
                ? 'Edit MCP OAuth client'
                : 'Configure MCP OAuth client'}</Button>
        <Button
            type="button"
            variant="outline"
            disabled={oauthDialogLoading || oauthConfigured !== true}
            class="cursor-pointer"
            href={`/api/oauth/start?source_id=${encodeURIComponent(data.source.id)}&flow=user_read&return_to=${encodeURIComponent(`/admin/settings/integrations/snowflake/${data.source.id}`)}`}
            >{data.actionAuth.authorized
                ? 'Re-authorize Snowflake'
                : 'Authorize Snowflake'}</Button>
    </Card.Footer>
</Card.Root>

{#if oauthDialogOpen && oauthProvider}
    <OAuthClientConfigDialog
        open={oauthDialogOpen}
        provider={oauthProvider.provider}
        displayName={oauthProvider.displayName}
        configured={oauthProvider.configured}
        registrationRequiresInitialAccessToken={data.oauth.registrationRequiresInitialAccessToken}
        config={oauthProvider.config}
        onSaved={() => {
            oauthDialogOpen = false
            oauthProvider = null
            void checkOAuth()
        }}
        onCancel={() => {
            oauthDialogOpen = false
            oauthProvider = null
        }} />
{/if}
