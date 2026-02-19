<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Input } from '$lib/components/ui/input'
    import { X, AlertCircle, Loader2 } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import jiraLogo from '$lib/images/icons/jira.svg'
    import type { JiraSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const config = (data.source.config as JiraSourceConfig) || {}

    let enabled = $state(data.source.isActive)
    let siteUrl = $state(config.base_url || '')
    let projectFilters = $state<string[]>(
        config.project_filters && Array.isArray(config.project_filters)
            ? config.project_filters
            : [],
    )
    let projectInput = $state('')

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let allProjects: { key: string; name: string }[] | null = null
    let suggestions = $state<{ key: string; name: string }[]>([])
    let showSuggestions = $state(false)
    let isLoadingProjects = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive
    let originalSiteUrl = siteUrl
    let originalProjectFilters: string[] = [...projectFilters]

    function addProject() {
        const project = projectInput.trim()
        if (project && !projectFilters.includes(project)) {
            projectFilters = [...projectFilters, project]
            projectInput = ''
        }
    }

    function removeProject(project: string) {
        projectFilters = projectFilters.filter((p) => p !== project)
    }

    function selectSuggestion(key: string) {
        if (!projectFilters.includes(key)) {
            projectFilters = [...projectFilters, key]
        }
        projectInput = ''
        suggestions = []
        showSuggestions = false
    }

    async function fetchProjects() {
        if (allProjects !== null) return
        isLoadingProjects = true
        try {
            const res = await fetch(`/api/sources/${data.source.id}/action`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    action: 'search_spaces',
                    params: { type: 'jira' },
                }),
            })
            if (res.ok) {
                const body = await res.json()
                allProjects = body.result ?? []
            }
        } catch {
            // Silently fail - user can still type manually
        } finally {
            isLoadingProjects = false
        }
    }

    function filterProjects(query: string) {
        if (!allProjects) return
        const q = query.trim().toLowerCase()
        if (!q) {
            suggestions = []
            showSuggestions = false
            return
        }
        suggestions = allProjects.filter(
            (p) =>
                (p.key.toLowerCase().includes(q) || p.name.toLowerCase().includes(q)) &&
                !projectFilters.includes(p.key),
        )
        showSuggestions = suggestions.length > 0
    }

    function validateForm() {
        formErrors = []
        return true
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
        const projectsChanged =
            JSON.stringify(projectFilters.sort()) !== JSON.stringify(originalProjectFilters.sort())

        hasUnsavedChanges =
            enabled !== originalEnabled || siteUrl !== originalSiteUrl || projectsChanged
    })
</script>

<svelte:head>
    <title>Configure Jira - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Configure Jira</h1>
            <p class="text-muted-foreground mt-2">Configure Jira indexing with project filtering</p>
        </div>

        {#if formErrors.length > 0}
            <Alert.Root variant="destructive" class="mb-6">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>Configuration Error</Alert.Title>
                <Alert.Description>
                    <ul class="list-inside list-disc">
                        {#each formErrors as error}
                            <li>{error}</li>
                        {/each}
                    </ul>
                </Alert.Description>
            </Alert.Root>
        {/if}

        <form
            method="POST"
            use:enhance={() => {
                if (!validateForm()) {
                    return async () => {}
                }
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
                                <img src={jiraLogo} alt="Jira" class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index issues, tickets, and project data
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
                    <div class="space-y-4">
                        <div class="space-y-2">
                            <Label for="siteUrl" class="text-sm font-medium">Site URL</Label>
                            <Input
                                id="siteUrl"
                                name="siteUrl"
                                type="url"
                                bind:value={siteUrl}
                                placeholder="https://your-domain.atlassian.net"
                                disabled={!enabled}
                                class="w-full" />
                        </div>

                        <div class="space-y-2 border-t pt-4">
                            <Label class="text-sm font-medium">Project Filters</Label>
                            <p class="text-muted-foreground text-xs">
                                Filter specific projects (leave empty for all projects)
                            </p>

                            <div class="relative">
                                <div class="flex gap-2">
                                    <Input
                                        bind:value={projectInput}
                                        placeholder="Search projects or enter key..."
                                        disabled={!enabled}
                                        class="flex-1"
                                        oninput={(e) => filterProjects(e.currentTarget.value)}
                                        onfocusout={() => {
                                            setTimeout(() => (showSuggestions = false), 200)
                                        }}
                                        onfocus={() => {
                                            fetchProjects()
                                            if (suggestions.length > 0) showSuggestions = true
                                        }}
                                        onkeydown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault()
                                                addProject()
                                            }
                                            if (e.key === 'Escape') {
                                                showSuggestions = false
                                            }
                                        }} />
                                    <Button
                                        type="button"
                                        variant="secondary"
                                        onclick={addProject}
                                        disabled={!enabled || !projectInput.trim()}>
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
                                                        onmousedown={() =>
                                                            selectSuggestion(suggestion.key)}>
                                                        <span class="font-medium"
                                                            >{suggestion.key}</span>
                                                        <span class="text-muted-foreground ml-2"
                                                            >{suggestion.name}</span>
                                                    </button>
                                                </li>
                                            {/each}
                                        </ul>
                                    </div>
                                {/if}
                                {#if isLoadingProjects}
                                    <div
                                        class="text-muted-foreground absolute top-2.5 right-16 text-xs">
                                        <Loader2 class="h-3 w-3 animate-spin" />
                                    </div>
                                {/if}
                            </div>

                            {#if projectFilters.length > 0}
                                <div class="flex flex-wrap gap-2">
                                    {#each projectFilters as project}
                                        <div
                                            class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                            <span>{project}</span>
                                            <button
                                                type="button"
                                                onclick={() => removeProject(project)}
                                                class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                                aria-label="Remove {project}">
                                                <X class="h-3 w-3" />
                                            </button>
                                        </div>
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    </div>

                    {#each projectFilters as project}
                        <input type="hidden" name="projectFilters" value={project} />
                    {/each}
                </Card.Content>
            </Card.Root>

            <div class="mt-8 flex justify-between">
                <Button variant="outline" href="/admin/settings/integrations">Cancel</Button>
                <Button
                    type="submit"
                    disabled={isSubmitting || !hasUnsavedChanges}
                    class="cursor-pointer">
                    {#if isSubmitting}
                        <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                    {/if}
                    Save Configuration
                </Button>
            </div>
        </form>
    </div>
</div>
