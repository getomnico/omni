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
    import confluenceLogo from '$lib/images/icons/confluence.svg'
    import type { ConfluenceSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const config = (data.source.config as ConfluenceSourceConfig) || {}

    let enabled = $state(data.source.isActive)
    let siteUrl = $state(config.base_url || '')
    let spaceFilters = $state<string[]>(
        config.space_filters && Array.isArray(config.space_filters) ? config.space_filters : [],
    )
    let spaceInput = $state('')

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalEnabled = data.source.isActive
    let originalSiteUrl = siteUrl
    let originalSpaceFilters: string[] = [...spaceFilters]

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
        const spacesChanged =
            JSON.stringify(spaceFilters.sort()) !== JSON.stringify(originalSpaceFilters.sort())

        hasUnsavedChanges =
            enabled !== originalEnabled || siteUrl !== originalSiteUrl || spacesChanged
    })
</script>

<svelte:head>
    <title>Configure Confluence - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Configure Confluence</h1>
            <p class="text-muted-foreground mt-2">
                Configure Confluence indexing with space filtering
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
            <Card.Root class="relative">
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title class="flex items-center gap-2">
                                <img src={confluenceLogo} alt="Confluence" class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index wiki pages, documentation, and spaces
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
                            <Label class="text-sm font-medium">Space Filters</Label>
                            <p class="text-muted-foreground text-xs">
                                Filter specific spaces (leave empty for all spaces)
                            </p>

                            <div class="flex gap-2">
                                <Input
                                    bind:value={spaceInput}
                                    placeholder="Enter space key..."
                                    disabled={!enabled}
                                    class="flex-1"
                                    onkeydown={(e) => {
                                        if (e.key === 'Enter') {
                                            e.preventDefault()
                                            addSpace()
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
                    </div>

                    {#each spaceFilters as space}
                        <input type="hidden" name="spaceFilters" value={space} />
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
