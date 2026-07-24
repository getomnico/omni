<script lang="ts">
    import { goto } from '$app/navigation'
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
    import { AuthType } from '$lib/types'
    import { toast } from 'svelte-sonner'

    type ProbeResult = {
        ok: boolean
        serverName: string | null
        serverVersion: string | null
        toolCount: number | null
        resourceCount: number | null
        suggestedSourceType: string | null
        oauth: unknown | null
        error?: string
    }

    let name = $state('')
    let endpointUrl = $state('')
    let sourceType = $state('')
    let authType = $state<string>('')
    let bearerToken = $state('')
    let writeToolsEnabled = $state(true)
    let probe = $state<ProbeResult | null>(null)
    let isTesting = $state(false)
    let isCreating = $state(false)

    const canCreate = $derived(
        Boolean(probe?.ok && name.trim() && endpointUrl.trim() && sourceType.trim()),
    )

    function payload(includeSecret = true) {
        return {
            name: name.trim(),
            endpointUrl: endpointUrl.trim(),
            sourceType: sourceType.trim(),
            authType: authType || null,
            bearerToken: includeSecret ? bearerToken : undefined,
            writeToolsEnabled,
        }
    }

    async function testConnection() {
        isTesting = true
        probe = null
        try {
            const response = await fetch('/api/remote-mcp/test', {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify(payload()),
            })
            const body = (await response.json().catch(() => null)) as ProbeResult | null
            if (!response.ok || !body?.ok) {
                throw new Error(body?.error || 'Remote MCP probe failed')
            }
            probe = body
            if (!sourceType && body.suggestedSourceType) sourceType = body.suggestedSourceType
            if (!name && body.serverName) name = body.serverName
            toast.success('Remote MCP connection succeeded')
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Remote MCP probe failed')
        } finally {
            isTesting = false
        }
    }

    async function create() {
        if (!canCreate) return
        isCreating = true
        try {
            const response = await fetch('/api/remote-mcp', {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify(payload()),
            })
            const body = await response.json().catch(() => null)
            if (!response.ok) throw new Error(body?.message || body?.error || 'Create failed')
            toast.success('Remote MCP server created')
            await goto(`/admin/settings/mcp/${body.id}`)
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Create failed')
        } finally {
            isCreating = false
        }
    }
</script>

<svelte:head><title>Add Remote MCP Server</title></svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-md space-y-6">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Add remote MCP server</h1>
            <p class="text-muted-foreground mt-2">
                Configure a remote Streamable HTTP MCP endpoint. Catalogs are discovered at runtime;
                secrets are write-only.
            </p>
        </div>

        <Card>
            <CardHeader>
                <CardTitle>Connection</CardTitle>
                <CardDescription>Only remote HTTP(S) MCP endpoints are supported.</CardDescription>
            </CardHeader>
            <CardContent class="space-y-5">
                <div class="space-y-2">
                    <Label for="name">Display name</Label>
                    <Input id="name" bind:value={name} placeholder="Acme MCP" />
                </div>
                <div class="space-y-2">
                    <Label for="endpoint">Endpoint URL</Label>
                    <Input
                        id="endpoint"
                        bind:value={endpointUrl}
                        placeholder="https://example.com/mcp" />
                </div>
                <div class="space-y-2">
                    <Label for="slug">App/source slug</Label>
                    <Input id="slug" bind:value={sourceType} placeholder="acme" />
                    <p class="text-muted-foreground text-xs">
                        Immutable after creation. Use lowercase letters, numbers, and underscores.
                    </p>
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
                        <Label for="token">Bearer token</Label>
                        <Input
                            id="token"
                            type="password"
                            bind:value={bearerToken}
                            placeholder="Paste token" />
                        <p class="text-muted-foreground text-xs">
                            Stored encrypted and never returned by the API.
                        </p>
                    </div>
                {/if}
                {#if authType === AuthType.OAUTH}
                    <p class="text-muted-foreground rounded-md border p-3 text-sm">
                        After creation, configure this server's OAuth client and authorize an admin
                        bootstrap credential on the edit page.
                    </p>
                {/if}
                <label class="flex items-center gap-2 text-sm">
                    <input type="checkbox" bind:checked={writeToolsEnabled} />
                    Expose write-capable tools
                </label>

                <div class="flex gap-2">
                    <Button variant="outline" onclick={testConnection} disabled={isTesting}
                        >{isTesting ? 'Testing...' : 'Test connection'}</Button>
                    <Button onclick={create} disabled={!canCreate || isCreating}
                        >{isCreating ? 'Creating...' : 'Create'}</Button>
                </div>

                {#if probe?.ok}
                    <div class="rounded-md border p-3 text-sm">
                        <div class="flex flex-wrap items-center gap-2">
                            <Badge variant="secondary">Connected</Badge>
                            {#if probe.serverName}<span>{probe.serverName}</span>{/if}
                            {#if probe.serverVersion}<span class="text-muted-foreground"
                                    >v{probe.serverVersion}</span
                                >{/if}
                        </div>
                        <div class="text-muted-foreground mt-2">
                            {probe.toolCount ?? 0} tools · {probe.resourceCount ?? 0} resources
                        </div>
                    </div>
                {/if}
            </CardContent>
        </Card>
    </div>
</div>
