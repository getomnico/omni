<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import { Input } from '$lib/components/ui/input'
    import { Loader2, X } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import { page } from '$app/state'
    import type { PageProps } from './$types'
    import clickupLogo from '$lib/images/icons/clickup.svg'
    import type { ClickUpSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const config = (data.source.config as ClickUpSourceConfig) || {}

    let enabled = $state(data.source.isActive)
    let spaceFilters = $state<string[]>(
        config.space_filters && Array.isArray(config.space_filters) ? config.space_filters : [],
    )
    let spaceInput = $state('')

    let isSubmitting = $state(false)
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let includeWritePermissions = $state(false)

    let allSpaces: { id: string; name: string; workspace_name?: string }[] | null = null
    let suggestions = $state<{ id: string; name: string; workspace_name?: string }[]>([])
    let showSuggestions = $state(false)
    let isLoadingSpaces = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive
    let originalSpaceFilters: string[] = [...spaceFilters]

    const actionAuthLabel = $derived(
        data.actionAuth.access === 'read_write'
            ? 'Authorized with read and write permissions'
            : data.actionAuth.access === 'read_only'
              ? 'Authorized with read-only permissions'
              : 'Not authorized',
    )

    const actionOAuthUrl = $derived.by(() => {
        const flow = includeWritePermissions ? 'user_write' : 'user_read'
        const returnTo = encodeURIComponent(page.url.pathname)
        return `/api/oauth/start?source_id=${data.source.id}&flow=${flow}&return_to=${returnTo}`
    })

    function addSpace() {
        const space = spaceInput.trim()
        if (space && !spaceFilters.includes(space)) {
            spaceFilters = [...spaceFilters, space]
            spaceInput = ''
        }
    }

    function removeSpace(space: string) {
        spaceFilters = spaceFilters.filter((s) => s !== space)
    }

    function selectSuggestion(id: string) {
        if (!spaceFilters.includes(id)) {
            spaceFilters = [...spaceFilters, id]
        }
        spaceInput = ''
        suggestions = []
        showSuggestions = false
    }

    async function fetchSpaces() {
        if (allSpaces !== null) return
        isLoadingSpaces = true
        try {
            const res = await fetch(`/api/sources/${data.source.id}/action`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    action: 'search_spaces',
                    params: {},
                }),
            })
            if (res.ok) {
                const body = await res.json()
                const result = body.result
                allSpaces = Array.isArray(result) ? result : (result?.spaces ?? [])
            }
        } catch {
            // Silently fail - user can still type space IDs manually.
        } finally {
            isLoadingSpaces = false
        }
    }

    function filterSpaces(query: string) {
        if (!allSpaces) return
        const q = query.trim().toLowerCase()
        if (!q) {
            suggestions = []
            showSuggestions = false
            return
        }
        suggestions = allSpaces.filter(
            (s) =>
                (s.id.toLowerCase().includes(q) ||
                    s.name.toLowerCase().includes(q) ||
                    (s.workspace_name ?? '').toLowerCase().includes(q)) &&
                !spaceFilters.includes(s.id),
        )
        showSuggestions = suggestions.length > 0
    }

    onMount(() => {
        beforeUnloadHandler = (e: BeforeUnloadEvent) => {
            if (hasUnsavedChanges && !skipUnsavedCheck) {
                e.preventDefault()
                e.returnValue = ''
            }
        }

        window.addEventListener('beforeunload', beforeUnloadHandler)

        return () => {
            if (beforeUnloadHandler) {
                window.removeEventListener('beforeunload', beforeUnloadHandler)
            }
        }
    })

    beforeNavigate(({ cancel }) => {
        if (hasUnsavedChanges && !skipUnsavedCheck) {
            const shouldLeave = confirm(
                'You have unsaved changes. Are you sure you want to leave this page?',
            )
            if (!shouldLeave) {
                cancel()
            }
        }
    })

    $effect(() => {
        const spacesChanged =
            JSON.stringify([...spaceFilters].sort()) !==
            JSON.stringify([...originalSpaceFilters].sort())

        hasUnsavedChanges = enabled !== originalEnabled || spacesChanged
    })
</script>

<svelte:head>
    <title>Configure ClickUp - {data.source.name}</title>
</svelte:head>
<form
    method="POST"
    use:enhance={() => {
        isSubmitting = true
        return async ({ result, update }) => {
            if (result.type === 'redirect') {
                skipUnsavedCheck = true
                hasUnsavedChanges = false

                if (beforeUnloadHandler) {
                    window.removeEventListener('beforeunload', beforeUnloadHandler)
                    beforeUnloadHandler = null
                }
            }

            await update()
            isSubmitting = false
        }
    }}>
    <Card.Root class="relative">
        <Card.Header>
            <div class="flex items-start justify-between">
                <div>
                    <Card.Title class="flex items-center gap-2">
                        <img src={clickupLogo} alt="ClickUp" class="h-5 w-5" />
                        {data.source.name}
                    </Card.Title>
                    <Card.Description class="mt-1">
                        Index tasks and docs from ClickUp workspaces
                    </Card.Description>
                </div>
                <div class="flex items-center gap-2">
                    <Label for="enabled" class="text-sm">Enabled</Label>
                    <Switch
                        id="enabled"
                        bind:checked={enabled}
                        name="enabled"
                        class="cursor-pointer" />
                </div>
            </div>
        </Card.Header>

        <Card.Content class="space-y-4">
            <div class="space-y-2">
                <Label class="text-sm font-medium">Space Filters</Label>
                <p class="text-muted-foreground text-xs">
                    Select specific ClickUp spaces to index. Leave empty to index all spaces.
                </p>

                <div class="relative">
                    <div class="flex gap-2">
                        <Input
                            bind:value={spaceInput}
                            placeholder="Search spaces or enter space ID..."
                            disabled={!enabled}
                            class="flex-1"
                            oninput={(e) => filterSpaces(e.currentTarget.value)}
                            onfocusout={() => {
                                setTimeout(() => (showSuggestions = false), 200)
                            }}
                            onfocus={() => {
                                fetchSpaces()
                                if (suggestions.length > 0) showSuggestions = true
                            }}
                            onkeydown={(e) => {
                                if (e.key === 'Enter') {
                                    e.preventDefault()
                                    addSpace()
                                }
                                if (e.key === 'Escape') {
                                    showSuggestions = false
                                }
                            }} />
                        <Button
                            type="button"
                            variant="secondary"
                            onclick={addSpace}
                            disabled={!enabled || !spaceInput.trim()}>
                            Add
                        </Button>
                    </div>
                    {#if showSuggestions}
                        <div
                            class="border-border bg-popover text-popover-foreground absolute z-10 mt-1 w-full rounded-md border shadow-md">
                            <ul class="max-h-48 overflow-y-auto py-1">
                                {#each suggestions as suggestion}
                                    <li>
                                        <button
                                            type="button"
                                            class="hover:bg-accent w-full px-3 py-2 text-left text-sm"
                                            onmousedown={() => selectSuggestion(suggestion.id)}>
                                            <span class="font-medium">{suggestion.name}</span>
                                            <span class="text-muted-foreground ml-2">
                                                {suggestion.id}
                                            </span>
                                            {#if suggestion.workspace_name}
                                                <span class="text-muted-foreground ml-2">
                                                    {suggestion.workspace_name}
                                                </span>
                                            {/if}
                                        </button>
                                    </li>
                                {/each}
                            </ul>
                        </div>
                    {/if}
                    {#if isLoadingSpaces}
                        <div class="text-muted-foreground absolute top-2.5 right-16 text-xs">
                            <Loader2 class="h-3 w-3 animate-spin" />
                        </div>
                    {/if}
                </div>

                {#if spaceFilters.length > 0}
                    <div class="flex flex-wrap gap-2">
                        {#each spaceFilters as space}
                            <div
                                class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                <span>{space}</span>
                                <button
                                    type="button"
                                    onclick={() => removeSpace(space)}
                                    class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                    aria-label="Remove {space}">
                                    <X class="h-3 w-3" />
                                </button>
                            </div>
                        {/each}
                    </div>
                {/if}
            </div>

            {#each spaceFilters as space}
                <input type="hidden" name="spaceFilters" value={space} />
            {/each}
        </Card.Content>
        <Card.Footer class="flex justify-end">
            <Button
                type="submit"
                disabled={isSubmitting || !hasUnsavedChanges}
                class="cursor-pointer">
                {#if isSubmitting}
                    <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                {/if}
                Save Configuration
            </Button>
        </Card.Footer>
    </Card.Root>
</form>

<Card.Root class="relative mt-4">
    <Card.Header>
        <Card.Title>ClickUp AI actions</Card.Title>
        <Card.Description>
            Indexing uses the API token configured for this source. AI actions use your own ClickUp
            OAuth authorization, so actions run as you and only have the permissions you grant.
        </Card.Description>
    </Card.Header>
    <Card.Content class="space-y-4">
        <div class="rounded-md border p-3 text-sm">
            <div class="font-medium">{actionAuthLabel}</div>
            {#if data.actionAuth.principalEmail}
                <div class="text-muted-foreground mt-1">
                    Authorized account: {data.actionAuth.principalEmail}
                </div>
            {:else}
                <div class="text-muted-foreground mt-1">
                    Authorize your ClickUp account to let Omni read ClickUp MCP resources and use
                    ClickUp actions in chat and agents.
                </div>
            {/if}
        </div>

        <div class="flex items-start gap-2">
            <Checkbox
                id="include-write-permissions"
                bind:checked={includeWritePermissions}
                class="mt-0.5 cursor-pointer" />
            <div class="space-y-1">
                <Label for="include-write-permissions" class="cursor-pointer">
                    Include write permissions
                </Label>
                <p class="text-muted-foreground text-sm">
                    Leave unchecked for read-only access. Check this to allow Omni to create or
                    update ClickUp tasks, comments, docs, time entries, and other writable objects
                    as you.
                </p>
            </div>
        </div>
    </Card.Content>
    <Card.Footer class="flex justify-end">
        <Button href={actionOAuthUrl} class="cursor-pointer">
            {data.actionAuth.authorized
                ? 'Update ClickUp authorization'
                : 'Authorize my ClickUp account'}
        </Button>
    </Card.Footer>
</Card.Root>
