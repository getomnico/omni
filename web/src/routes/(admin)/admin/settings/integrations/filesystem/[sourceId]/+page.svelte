<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { ArrowLeft, AlertCircle, Loader2, HardDrive, Trash2 } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type { FilesystemSourceConfig } from '$lib/types'
    import FilesystemConnectorForm from '$lib/components/filesystem-connector-form.svelte'

    let { data }: PageProps = $props()

    const getConfig = (): Partial<FilesystemSourceConfig> => {
        if (!data.source?.config) return {}
        return typeof data.source.config === 'string'
            ? JSON.parse(data.source.config)
            : data.source.config
    }

    const config = getConfig()

    let filesystemEnabled = $state(data.source ? data.source.isActive : false)
    let name = $state(data.source?.name || '')
    let basePath = $state(config.base_path || '')
    let fileExtensions = $state<string[]>(
        config.file_extensions && Array.isArray(config.file_extensions)
            ? config.file_extensions
            : [],
    )
    let excludePatterns = $state<string[]>(
        config.exclude_patterns && Array.isArray(config.exclude_patterns)
            ? config.exclude_patterns
            : [],
    )
    let maxFileSizeMb = $state(
        config.max_file_size_bytes ? Math.round(config.max_file_size_bytes / (1024 * 1024)) : 10,
    )
    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let showRemoveDialog = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalFilesystemEnabled = data.source ? data.source.isActive : false
    let originalBasePath = basePath
    let originalFileExtensions: string[] = [...fileExtensions]
    let originalExcludePatterns: string[] = [...excludePatterns]
    let originalMaxFileSizeMb = maxFileSizeMb

    function validateForm() {
        formErrors = []

        if (filesystemEnabled && !basePath.trim()) {
            formErrors = [
                ...formErrors,
                'Base path is required when filesystem indexing is enabled',
            ]
            return false
        }

        if (filesystemEnabled && !basePath.startsWith('/')) {
            formErrors = [...formErrors, 'Base path must be an absolute path (starting with /)']
            return false
        }

        if (maxFileSizeMb < 1) {
            formErrors = [...formErrors, 'Max file size must be at least 1 MB']
            return false
        }

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
        const extensionsChanged =
            JSON.stringify(fileExtensions.sort()) !== JSON.stringify(originalFileExtensions.sort())

        const patternsChanged =
            JSON.stringify(excludePatterns.sort()) !==
            JSON.stringify(originalExcludePatterns.sort())

        hasUnsavedChanges =
            filesystemEnabled !== originalFilesystemEnabled ||
            basePath !== originalBasePath ||
            maxFileSizeMb !== originalMaxFileSizeMb ||
            extensionsChanged ||
            patternsChanged
    })
</script>

<svelte:head>
    <title>Configure Filesystem - {data.source.name}</title>
</svelte:head>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-4">
        <a
            href="/admin/settings/integrations"
            class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors">
            <ArrowLeft class="h-4 w-4" />
            Back to Integrations
        </a>

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
                                <HardDrive class="h-5 w-5" />
                                {data.source.name}
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index files from local directories
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="filesystemEnabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="filesystemEnabled"
                                bind:checked={filesystemEnabled}
                                name="filesystemEnabled"
                                class="cursor-pointer" />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content>
                    <FilesystemConnectorForm
                        bind:name
                        bind:basePath
                        bind:fileExtensions
                        bind:excludePatterns
                        bind:maxFileSizeMb
                        disabled={!filesystemEnabled} />
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

        <Card.Root>
            <Card.Content class="flex items-center justify-between">
                <div>
                    <Card.Title>Delete Source</Card.Title>
                    <Card.Description>
                        Permanently delete this source and all its synced documents, credentials,
                        and sync history.
                    </Card.Description>
                </div>
                <Button
                    variant="destructive"
                    class="cursor-pointer"
                    onclick={() => (showRemoveDialog = true)}>
                    <Trash2 class="mr-2 h-4 w-4" />
                    Delete Permanently
                </Button>
            </Card.Content>
        </Card.Root>

        <RemoveSourceDialog
            bind:open={showRemoveDialog}
            sourceId={data.source.id}
            sourceName={data.source.name} />
    </div>
</div>
