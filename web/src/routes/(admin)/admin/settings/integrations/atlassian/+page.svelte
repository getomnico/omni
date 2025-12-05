<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Input } from '$lib/components/ui/input'
    import { Badge } from '$lib/components/ui/badge'
    import { X, AlertCircle, Loader2 } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import jiraLogo from '$lib/images/icons/jira.svg'
    import confluenceLogo from '$lib/images/icons/confluence.svg'

    let { data }: PageProps = $props()

    // Helper to parse config
    const getJiraConfig = () => {
        if (!data.jiraSource?.config) return {}
        return typeof data.jiraSource.config === 'string'
            ? JSON.parse(data.jiraSource.config)
            : data.jiraSource.config
    }

    const getConfluenceConfig = () => {
        if (!data.confluenceSource?.config) return {}
        return typeof data.confluenceSource.config === 'string'
            ? JSON.parse(data.confluenceSource.config)
            : data.confluenceSource.config
    }

    const jiraConfig = getJiraConfig()
    const confluenceConfig = getConfluenceConfig()

    let jiraEnabled = $state(data.jiraSource ? data.jiraSource.isActive : false)
    let confluenceEnabled = $state(data.confluenceSource ? data.confluenceSource.isActive : false)

    let jiraApiToken = $state('')
    let jiraSiteUrl = $state(jiraConfig.siteUrl || '')
    let jiraProjectFilters = $state<string[]>(
        jiraConfig.projectFilters && Array.isArray(jiraConfig.projectFilters)
            ? jiraConfig.projectFilters
            : [],
    )
    let jiraProjectInput = $state('')

    let confluenceApiToken = $state('')
    let confluenceSiteUrl = $state(confluenceConfig.siteUrl || '')
    let confluenceSpaceFilters = $state<string[]>(
        confluenceConfig.spaceFilters && Array.isArray(confluenceConfig.spaceFilters)
            ? confluenceConfig.spaceFilters
            : [],
    )
    let confluenceSpaceInput = $state('')

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalJiraEnabled = data.jiraSource ? data.jiraSource.isActive : false
    let originalConfluenceEnabled = data.confluenceSource ? data.confluenceSource.isActive : false
    let originalJiraProjectFilters: string[] = [...jiraProjectFilters]
    let originalConfluenceSpaceFilters: string[] = [...confluenceSpaceFilters]

    function addJiraProject() {
        const project = jiraProjectInput.trim()
        if (project && !jiraProjectFilters.includes(project)) {
            jiraProjectFilters = [...jiraProjectFilters, project]
            jiraProjectInput = ''
        }
    }

    function removeJiraProject(project: string) {
        jiraProjectFilters = jiraProjectFilters.filter((p) => p !== project)
    }

    function addConfluenceSpace() {
        const space = confluenceSpaceInput.trim()
        if (space && !confluenceSpaceFilters.includes(space)) {
            confluenceSpaceFilters = [...confluenceSpaceFilters, space]
            confluenceSpaceInput = ''
        }
    }

    function removeConfluenceSpace(space: string) {
        confluenceSpaceFilters = confluenceSpaceFilters.filter((s) => s !== space)
    }

    function validateForm() {
        formErrors = []
        // TODO: Add validations

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
        const jiraProjectsChanged =
            JSON.stringify(jiraProjectFilters.sort()) !==
            JSON.stringify(originalJiraProjectFilters.sort())
        const confluenceSpacesChanged =
            JSON.stringify(confluenceSpaceFilters.sort()) !==
            JSON.stringify(originalConfluenceSpaceFilters.sort())

        hasUnsavedChanges =
            jiraEnabled !== originalJiraEnabled ||
            confluenceEnabled !== originalConfluenceEnabled ||
            jiraProjectsChanged ||
            confluenceSpacesChanged ||
            jiraApiToken !== '' ||
            confluenceApiToken !== ''
    })
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Configure Atlassian</h1>
            <p class="text-muted-foreground mt-2">
                Configure JIRA and Confluence services with project and space filtering
            </p>
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
            <div class="grid gap-6 md:grid-cols-2">
                <!-- JIRA Configuration -->
                <Card.Root class="relative">
                    <Card.Header>
                        <div class="flex items-start justify-between">
                            <div>
                                <Card.Title class="flex items-center gap-2">
                                    <img src={jiraLogo} alt="JIRA" class="h-5 w-5" />
                                    JIRA
                                </Card.Title>
                                <Card.Description class="mt-1">
                                    Index issues, tickets, and project data
                                </Card.Description>
                            </div>
                            <div class="flex items-center gap-2">
                                <Label for="jiraEnabled" class="text-sm">Enabled</Label>
                                <Switch
                                    id="jiraEnabled"
                                    bind:checked={jiraEnabled}
                                    name="jiraEnabled"
                                    class="cursor-pointer" />
                            </div>
                        </div>
                    </Card.Header>

                    <Card.Content class="space-y-4">
                        <div class="space-y-4">
                            <div class="space-y-2">
                                <Label for="jiraSiteUrl" class="text-sm font-medium"
                                    >Site URL</Label>
                                <Input
                                    id="jiraSiteUrl"
                                    name="jiraSiteUrl"
                                    type="url"
                                    bind:value={jiraSiteUrl}
                                    placeholder="https://your-domain.atlassian.net"
                                    disabled={!jiraEnabled}
                                    class="w-full" />
                            </div>

                            <div class="space-y-2">
                                <Label for="jiraApiToken" class="text-sm font-medium">
                                    API Token
                                </Label>
                                <Input
                                    id="jiraApiToken"
                                    name="jiraApiToken"
                                    type="password"
                                    bind:value={jiraApiToken}
                                    placeholder="Enter API token (leave blank to keep existing)"
                                    disabled={!jiraEnabled}
                                    class="w-full" />
                            </div>

                            <div class="space-y-2 border-t pt-4">
                                <Label class="text-sm font-medium">Project Filters</Label>
                                <p class="text-muted-foreground text-xs">
                                    Filter specific projects (leave empty for all projects)
                                </p>

                                <div class="flex gap-2">
                                    <Input
                                        bind:value={jiraProjectInput}
                                        placeholder="Enter project key..."
                                        disabled={!jiraEnabled}
                                        class="flex-1"
                                        onkeydown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault()
                                                addJiraProject()
                                            }
                                        }} />
                                    <Button
                                        type="button"
                                        variant="secondary"
                                        onclick={addJiraProject}
                                        disabled={!jiraEnabled || !jiraProjectInput.trim()}>
                                        Add
                                    </Button>
                                </div>

                                {#if jiraProjectFilters.length > 0}
                                    <div class="flex flex-wrap gap-2">
                                        {#each jiraProjectFilters as project}
                                            <div
                                                class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                                <span>{project}</span>
                                                <button
                                                    type="button"
                                                    onclick={() => removeJiraProject(project)}
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

                        {#each jiraProjectFilters as project}
                            <input type="hidden" name="jiraProjectFilters" value={project} />
                        {/each}
                    </Card.Content>
                </Card.Root>

                <!-- Confluence Configuration -->
                <Card.Root class="relative">
                    <Card.Header>
                        <div class="flex items-start justify-between">
                            <div>
                                <Card.Title class="flex items-center gap-2">
                                    <img src={confluenceLogo} alt="Confluence" class="h-5 w-5" />
                                    Confluence
                                </Card.Title>
                                <Card.Description class="mt-1">
                                    Index wiki pages, documentation, and spaces
                                </Card.Description>
                            </div>
                            <div class="flex items-center gap-2">
                                <Label for="confluenceEnabled" class="text-sm">Enabled</Label>
                                <Switch
                                    id="confluenceEnabled"
                                    bind:checked={confluenceEnabled}
                                    name="confluenceEnabled"
                                    class="cursor-pointer" />
                            </div>
                        </div>
                    </Card.Header>

                    <Card.Content class="space-y-4">
                        <div class="space-y-4">
                            <div class="space-y-2">
                                <Label for="confluenceSiteUrl" class="text-sm font-medium">
                                    Site URL
                                </Label>
                                <Input
                                    id="confluenceSiteUrl"
                                    name="confluenceSiteUrl"
                                    type="url"
                                    bind:value={confluenceSiteUrl}
                                    placeholder="https://your-domain.atlassian.net"
                                    disabled={!confluenceEnabled}
                                    class="w-full" />
                            </div>

                            <div class="space-y-2">
                                <Label for="confluenceApiToken" class="text-sm font-medium">
                                    API Token
                                </Label>
                                <Input
                                    id="confluenceApiToken"
                                    name="confluenceApiToken"
                                    type="password"
                                    bind:value={confluenceApiToken}
                                    placeholder="Enter API token (leave blank to keep existing)"
                                    disabled={!confluenceEnabled}
                                    class="w-full" />
                            </div>

                            <div class="space-y-2 border-t pt-4">
                                <Label class="text-sm font-medium">Space Filters</Label>
                                <p class="text-muted-foreground text-xs">
                                    Filter specific spaces (leave empty for all spaces)
                                </p>

                                <div class="flex gap-2">
                                    <Input
                                        bind:value={confluenceSpaceInput}
                                        placeholder="Enter space key..."
                                        disabled={!confluenceEnabled}
                                        class="flex-1"
                                        onkeydown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault()
                                                addConfluenceSpace()
                                            }
                                        }} />
                                    <Button
                                        type="button"
                                        variant="secondary"
                                        onclick={addConfluenceSpace}
                                        disabled={!confluenceEnabled ||
                                            !confluenceSpaceInput.trim()}>
                                        Add
                                    </Button>
                                </div>

                                {#if confluenceSpaceFilters.length > 0}
                                    <div class="flex flex-wrap gap-2">
                                        {#each confluenceSpaceFilters as space}
                                            <div
                                                class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                                                <span>{space}</span>
                                                <button
                                                    type="button"
                                                    onclick={() => removeConfluenceSpace(space)}
                                                    class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                                                    aria-label="Remove {space}">
                                                    <X class="h-3 w-3" />
                                                </button>
                                            </div>
                                        {/each}
                                    </div>
                                {/if}
                            </div>
                        </div>

                        {#each confluenceSpaceFilters as space}
                            <input type="hidden" name="confluenceSpaceFilters" value={space} />
                        {/each}
                    </Card.Content>
                </Card.Root>
            </div>

            <!-- Submit buttons -->
            <div class="mt-8 flex justify-between">
                <Button variant="outline" href="/admin/settings/integrations">Cancel</Button>
                <Button type="submit" disabled={isSubmitting || !hasUnsavedChanges}>
                    {#if isSubmitting}
                        <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                    {/if}
                    Save Configuration
                </Button>
            </div>
        </form>
    </div>
</div>
