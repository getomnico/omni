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
    import * as Select from '$lib/components/ui/select'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import OAuthClientConfigDialog from '$lib/components/oauth-integrations/oauth-client-config-dialog.svelte'
    import { AuthType } from '$lib/types'
    import { RefreshCw } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()
    const source = $derived(data.source)

    const initialName = data.source.name
    const initialEndpointUrl = String(
        (data.source.config as Record<string, unknown>).endpoint_url ?? '',
    )
    const initialAuthType: string = data.source.authType ?? ''
    const initialWriteToolsEnabled =
        (data.source.config as Record<string, unknown>).write_tools_enabled !== false

    let name = $state(initialName)
    let endpointUrl = $state(initialEndpointUrl)
    let authType = $state<string>(initialAuthType)
    let bearerToken = $state('')
    let writeToolsEnabled = $state(initialWriteToolsEnabled)
    let isTesting = $state(false)
    let isSaving = $state(false)
    let isDeleting = $state(false)
    let oauthDialogOpen = $state(false)
    let probeSummary = $state<string | null>(null)

    const hasChanges = $derived(
        name !== initialName ||
            endpointUrl !== initialEndpointUrl ||
            authType !== initialAuthType ||
            writeToolsEnabled !== initialWriteToolsEnabled ||
            Boolean(bearerToken),
    )

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
        if (!hasChanges) return
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
                <div class="flex items-start justify-between gap-4">
                    <div>
                        <CardTitle>Connection</CardTitle>
                    </div>
                    <div class="flex shrink-0 items-center gap-2 text-sm">
                        <Badge variant={data.manifest.available ? 'secondary' : 'outline'}>
                            {data.manifest.available ? 'Available' : 'Unavailable'}
                        </Badge>
                        <span class="text-muted-foreground">
                            {data.manifest.toolCount} tools · {data.manifest.resourceCount}{' '}
                            resources
                        </span>
                        {#if probeSummary}
                            <span class="text-muted-foreground">{probeSummary}</span>
                        {/if}
                        <Tooltip.Root>
                            <Tooltip.Trigger>
                                <Button
                                    variant="ghost"
                                    size="sm"
                                    onclick={testConnection}
                                    disabled={isTesting}
                                    class="cursor-pointer">
                                    <RefreshCw
                                        class="h-3.5 w-3.5 {isTesting ? 'animate-spin' : ''}" />
                                </Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content side="bottom">Test connection</Tooltip.Content>
                        </Tooltip.Root>
                    </div>
                </div>
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
                    <Select.Root type="single" bind:value={authType}>
                        <Select.Trigger id="auth" class="w-full cursor-pointer">
                            {authType === AuthType.BEARER_TOKEN
                                ? 'Shared bearer token'
                                : authType === AuthType.OAUTH
                                  ? 'Per-user OAuth'
                                  : 'Public / no credentials'}
                        </Select.Trigger>
                        <Select.Content>
                            <Select.Item value="" class="cursor-pointer"
                                >Public / no credentials</Select.Item>
                            <Select.Item value={AuthType.BEARER_TOKEN} class="cursor-pointer"
                                >Shared bearer token</Select.Item>
                            <Select.Item value={AuthType.OAUTH} class="cursor-pointer"
                                >Per-user OAuth</Select.Item>
                        </Select.Content>
                    </Select.Root>
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
                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        onclick={save}
                        disabled={!hasChanges || isSaving}
                        class="cursor-pointer">
                        {isSaving ? 'Saving...' : 'Save'}
                    </Button>
                    <Button
                        variant="destructive"
                        onclick={deleteSource}
                        disabled={isDeleting}
                        class="cursor-pointer">
                        {isDeleting ? 'Deleting...' : 'Delete'}
                    </Button>
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
