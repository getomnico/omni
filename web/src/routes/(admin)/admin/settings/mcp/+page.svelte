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
    import * as Dialog from '$lib/components/ui/dialog'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Badge } from '$lib/components/ui/badge'
    import { AuthType } from '$lib/types'
    import { Plus, Server } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'
    import type { PageProps } from './$types'

    let { data }: PageProps = $props()

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

    let sources = $state(data.sources)
    let manifestBySourceType = $state(
        data.manifestBySourceType as Record<string, { toolCount: number; resourceCount: number }>,
    )

    // Dialog form state
    let dialogOpen = $state(false)
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

    function resetForm() {
        name = ''
        endpointUrl = ''
        sourceType = ''
        authType = ''
        bearerToken = ''
        writeToolsEnabled = true
        probe = null
        isTesting = false
        isCreating = false
    }

    function openDialog() {
        resetForm()
        dialogOpen = true
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
            dialogOpen = false
            resetForm()
            await goto(`/admin/settings/mcp/${body.id}`)
        } catch (error) {
            toast.error(error instanceof Error ? error.message : 'Create failed')
        } finally {
            isCreating = false
        }
    }

    function authLabel(authType: string | null): string {
        if (authType === AuthType.BEARER_TOKEN) return 'Bearer'
        if (authType === AuthType.OAUTH) return 'OAuth'
        return 'Public'
    }
</script>

<svelte:head><title>Remote MCP Servers</title></svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-md space-y-6">
        <div class="flex items-center justify-between gap-4">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">Remote MCP Servers</h1>
                <p class="text-muted-foreground mt-2">
                    Connect to remote MCP servers. Available tools and resources are detected
                    automatically.
                </p>
            </div>
            <Button onclick={openDialog} class="cursor-pointer">
                <Plus class="h-4 w-4" />
                Add MCP server
            </Button>
        </div>

        {#if sources.length > 0}
            <div class="space-y-2">
                {#each sources as source}
                    {@const m = manifestBySourceType[source.sourceType]}
                    <button
                        onclick={() => goto(`/admin/settings/mcp/${source.id}`)}
                        class="bg-card hover:bg-accent flex w-full items-center justify-between gap-4 rounded-lg border px-4 py-3 text-left transition-colors">
                        <div class="min-w-0 flex-1 space-y-1">
                            <div class="flex flex-wrap items-center gap-2">
                                <Server class="h-4 w-4 shrink-0" />
                                <span class="font-medium">{source.name}</span>
                                <Badge variant="outline">{source.sourceType}</Badge>
                                <Badge variant={m ? 'secondary' : 'outline'}>
                                    {m ? 'Available' : 'Unavailable'}
                                </Badge>
                                <Badge variant="outline">{authLabel(source.authType)}</Badge>
                                {#if !(source.config as Record<string, unknown>).write_tools_enabled}
                                    <Badge variant="outline">Read-only</Badge>
                                {/if}
                            </div>
                            <div class="text-muted-foreground text-xs">
                                {m
                                    ? `${m.toolCount} tools · ${m.resourceCount} resources`
                                    : 'Not yet discovered'}
                                {#if !source.isActive}
                                    · <span class="text-amber-500">Inactive</span>
                                {/if}
                            </div>
                        </div>
                    </button>
                {/each}
            </div>
        {:else}
            <div class="rounded-lg border border-dashed py-12 text-center">
                <Server class="text-muted-foreground mx-auto h-8 w-8" />
                <p class="text-muted-foreground mt-3 text-sm">
                    No remote MCP servers configured yet.
                </p>
                <Button variant="outline" onclick={openDialog} class="mt-4 cursor-pointer">
                    <Plus class="h-4 w-4" />
                    Add your first MCP server
                </Button>
            </div>
        {/if}
    </div>
</div>

<Dialog.Root
    open={dialogOpen}
    onOpenChange={(o) => {
        dialogOpen = o
        if (!o) resetForm()
    }}>
    <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Content
            class="bg-background fixed top-[50%] left-[50%] z-50 max-h-[85vh] w-full max-w-2xl translate-x-[-50%] translate-y-[-50%] overflow-y-auto rounded-lg border p-6 shadow-lg">
            <Dialog.Header>
                <Dialog.Title>Add remote MCP server</Dialog.Title>
                <Dialog.Description>
                    Connect to a remote MCP server. Only HTTP or HTTPS URLs are supported.
                </Dialog.Description>
            </Dialog.Header>

            <div class="space-y-5 py-4">
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
                        Can't be changed later. Use 2-50 lowercase letters, numbers, hyphens, or
                        underscores. Must start with a letter.
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
                    </div>
                {/if}
                {#if authType === AuthType.OAUTH}
                    <p class="text-muted-foreground rounded-md border p-3 text-sm">
                        After creation, configure OAuth and authorize admin access on the edit page.
                    </p>
                {/if}
                <label class="flex items-center gap-2 text-sm">
                    <input type="checkbox" bind:checked={writeToolsEnabled} />
                    Expose write-capable tools
                </label>

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
            </div>

            <Dialog.Footer class="flex gap-2">
                <Button variant="outline" onclick={testConnection} disabled={isTesting}
                    >{isTesting ? 'Testing...' : 'Test connection'}</Button>
                <Button onclick={create} disabled={!canCreate || isCreating}
                    >{isCreating ? 'Creating...' : 'Create'}</Button>
            </Dialog.Footer>
        </Dialog.Content>
    </Dialog.Portal>
</Dialog.Root>
