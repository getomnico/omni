<script lang="ts">
    import { goto, invalidateAll } from '$app/navigation'
    import { page } from '$app/state'
    import { Button } from '$lib/components/ui/button'
    import {
        Card,
        CardContent,
        CardDescription,
        CardHeader,
        CardTitle,
    } from '$lib/components/ui/card'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Badge } from '$lib/components/ui/badge'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()
    const source = $derived(data.source)

    let name = $state(data.source.name)
    let endpointUrl = $state(
        String((data.source.config as Record<string, unknown>).endpoint_url ?? ''),
    )
    let authType = $state<string>(data.source.authType ?? '')
    let bearerToken = $state('')
    let writeToolsEnabled = $state(
        (data.source.config as Record<string, unknown>).write_tools_enabled !== false,
    )
    let isTesting = $state(false)
    let isSaving = $state(false)
    let isDeleting = $state(false)
    let oauthDialogOpen = $state(false)
    let probeSummary = $state<string | null>(null)

    function payload(includeSecret = true) {
        return {
            name: name.trim(),
            sourceType: source.sourceType,
            endpointUrl: endpointUrl.trim(),
            authType: authType || null,
            bearerToken: includeSecret ? bearerToken : undefined,
            sourceId: source.id,
            writeToolsEnabled,
        }
    }

    async function testConnection() {
        isTesting = true
        probeSummary = null
        try {
            const response = await fetch('/api/remote-mcp/test', {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify(payload(!!bearerToken)),
            })
            const body = await response.json().catch(() => null)
            if (!response.ok || !body?.ok) throw new Error(body?.error || 'Probe failed')
            probeSummary = `${body.toolCount ?? 0} tools · ${body.resourceCount ?? 0} resources`
            toast.success('Remote MCP connection succeeded')
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Probe failed')
        } finally {
            isTesting = false
        }
    }

    async function save() {
        isSaving = true
        try {
            const response = await fetch(`/api/remote-mcp/${source.id}`, {
                method: 'PUT',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify(payload(!!bearerToken)),
            })
            const body = await response.json().catch(() => null)
            if (!response.ok) throw new Error(body?.message || body?.error || 'Save failed')
            bearerToken = ''
            probeSummary = `${body.probe?.toolCount ?? 0} tools · ${body.probe?.resourceCount ?? 0} resources`
            toast.success('Remote MCP server updated')
            await invalidateAll()
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Save failed')
        } finally {
            isSaving = false
        }
    }

    async function startOAuth() {
        const returnTo = encodeURIComponent(page.url.pathname)
        window.location.href = `/api/oauth/start?source_id=${source.id}&flow=org_source&return_to=${returnTo}`
    }

    async function deleteSource() {
        if (
            !confirm(
                `Delete ${source.name}? This permanently removes the connection and all stored credentials.`,
            )
        )
            return
        isDeleting = true
        try {
            const response = await fetch(`/api/remote-mcp/${source.id}`, { method: 'DELETE' })
            const body = await response.json().catch(() => null)
            if (!response.ok) throw new Error(body?.message || body?.error || 'Delete failed')
            toast.success('Remote MCP server deleted')
            await goto('/admin/settings/mcp')
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Delete failed')
        } finally {
            isDeleting = false
        }
    }
</script>

<svelte:head><title>{source.name} - Remote MCP</title></svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-md space-y-6">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Edit remote MCP server</h1>
            <p class="text-muted-foreground mt-2">
                {source.name} · <code>{source.sourceType}</code>
            </p>
        </div>

        <Card>
            <CardHeader>
                <CardTitle>Connection status</CardTitle>
                <CardDescription
                    >Available tools and resources are detected automatically.</CardDescription>
            </CardHeader>
            <CardContent class="flex flex-wrap items-center gap-3 text-sm">
                <Badge variant={data.manifest.available ? 'secondary' : 'outline'}
                    >{data.manifest.available ? 'Available' : 'Unavailable'}</Badge>
                <span
                    >{data.manifest.toolCount} tools · {data.manifest.resourceCount} resources</span>
                {#if probeSummary}<span class="text-muted-foreground"
                        >Latest probe: {probeSummary}</span
                    >{/if}
                <Button variant="outline" size="sm" onclick={testConnection} disabled={isTesting}
                    >{isTesting ? 'Refreshing...' : 'Test / refresh'}</Button>
            </CardContent>
        </Card>

        <Card>
            <CardHeader>
                <CardTitle>Connection</CardTitle>
                <CardDescription>The connection ID can't be changed later.</CardDescription>
            </CardHeader>
            <CardContent class="space-y-5">
                <div class="space-y-2">
                    <Label for="name">Display name</Label>
                    <Input id="name" bind:value={name} />
                </div>
                <div class="space-y-2">
                    <Label for="slug">App/source slug</Label>
                    <Input id="slug" value={source.sourceType} disabled />
                </div>
                <div class="space-y-2">
                    <Label for="endpoint">Endpoint URL</Label>
                    <Input id="endpoint" bind:value={endpointUrl} />
                </div>
                <div class="space-y-2">
                    <Label for="auth">Authentication</Label>
                    <select
                        id="auth"
                        bind:value={authType}
                        class="border-input bg-background w-full rounded-md border px-3 py-2 text-sm">
                        <option value="">Public / no credentials</option>
                        <option value={AuthType.BEARER_TOKEN}>Shared bearer token</option>
                        <option value={AuthType.OAUTH}>Per-user OAuth</option>
                    </select>
                </div>
                {#if authType === AuthType.BEARER_TOKEN}
                    <div class="space-y-2">
                        <Label for="token">Replace bearer token</Label>
                        <Input
                            id="token"
                            type="password"
                            bind:value={bearerToken}
                            placeholder="Leave blank to keep existing token" />
                    </div>
                {/if}
                <label class="flex items-center gap-2 text-sm">
                    <input type="checkbox" bind:checked={writeToolsEnabled} />
                    Expose write-capable tools
                </label>
                <div class="flex flex-wrap gap-2">
                    <Button onclick={save} disabled={isSaving}
                        >{isSaving ? 'Saving...' : 'Save and test'}</Button>
                    <Button variant="outline" onclick={testConnection} disabled={isTesting}
                        >Test only</Button>
                    <Button variant="destructive" onclick={deleteSource} disabled={isDeleting}
                        >{isDeleting ? 'Deleting...' : 'Delete'}</Button>
                </div>
            </CardContent>
        </Card>

        {#if authType === AuthType.OAUTH}
            <Card>
                <CardHeader>
                    <CardTitle>OAuth</CardTitle>
                    <CardDescription
                        >OAuth settings for this connection: <code>{data.oauth.provider}</code
                        >.</CardDescription>
                </CardHeader>
                <CardContent class="space-y-4">
                    <div class="flex flex-wrap items-center gap-2 text-sm">
                        <Badge variant={data.oauth.configured ? 'secondary' : 'outline'}
                            >{data.oauth.configured
                                ? 'OAuth configured'
                                : 'OAuth not configured'}</Badge>
                        <span>Each user authorizes individually before using protected tools.</span>
                    </div>
                    <div class="flex flex-wrap gap-2">
                        <Button variant="outline" onclick={() => (oauthDialogOpen = true)}
                            >{data.oauth.configured
                                ? 'Edit OAuth settings'
                                : 'Add OAuth settings'}</Button>
                        <Button onclick={startOAuth} disabled={!data.oauth.configured}
                            >Authorize admin access</Button>
                    </div>
                </CardContent>
            </Card>
        {/if}
    </div>
</div>

<OAuthClientConfigDialog
    open={oauthDialogOpen}
    provider={data.oauth.provider}
    displayName={`${source.name} MCP`}
    configured={data.oauth.configured}
    config={data.oauth.config}
    onSaved={() => (oauthDialogOpen = false)}
    onCancel={() => (oauthDialogOpen = false)} />
