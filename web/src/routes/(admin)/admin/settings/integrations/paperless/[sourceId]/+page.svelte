<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { ArrowLeft, AlertCircle, Loader2 } from '@lucide/svelte'
    import RemoveSourceDialog from '../../remove-source-dialog.svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type { PaperlessNgxSourceConfig } from '$lib/types'

    let { data }: PageProps = $props()

    const cfg = (data.source.config as Partial<PaperlessNgxSourceConfig>) ?? {}

    let enabled = $state(data.source.isActive)
    let baseUrl = $state(cfg.base_url ?? '')
    let apiKey = $state('')

    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)
    let showRemoveDialog = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    const origEnabled = data.source.isActive
    const origBaseUrl = cfg.base_url ?? ''

    $effect(() => {
        hasUnsavedChanges =
            enabled !== origEnabled || baseUrl !== origBaseUrl || apiKey !== ''
    })

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
            if (!shouldLeave) cancel()
        }
    })

    function validateForm(): boolean {
        formErrors = []
        if (!baseUrl.trim()) {
            formErrors = [...formErrors, 'Paperless-ngx URL is required']
        }
        return formErrors.length === 0
    }
</script>

<svelte:head>
    <title>Configure Paperless-ngx - {data.source.name}</title>
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
            <Alert.Root variant="destructive">
                <AlertCircle class="h-4 w-4" />
                <Alert.Title>Configuration Error</Alert.Title>
                <Alert.Description>
                    <ul class="list-inside list-disc">
                        {#each formErrors as err}
                            <li>{err}</li>
                        {/each}
                    </ul>
                </Alert.Description>
            </Alert.Root>
        {/if}

        <form
            method="POST"
            use:enhance={() => {
                if (!validateForm()) return async () => {}
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
            <Card.Root>
                <Card.Header>
                    <div class="flex items-start justify-between">
                        <div>
                            <Card.Title>{data.source.name}</Card.Title>
                            <Card.Description class="mt-1">
                                Index documents and OCR content from paperless-ngx
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

                <Card.Content class="space-y-6">
                    <!-- Connection -->
                    <div class="space-y-4">
                        <h3 class="text-sm font-semibold">Connection</h3>

                        <div class="space-y-1.5">
                            <Label for="base_url">Paperless-ngx URL</Label>
                            <Input
                                id="base_url"
                                name="base_url"
                                bind:value={baseUrl}
                                placeholder="https://paperless.example.com"
                                required />
                            <p class="text-muted-foreground text-xs">
                                The base URL of your paperless-ngx instance.
                            </p>
                        </div>
                    </div>

                    <!-- Credentials -->
                    <div class="space-y-4 border-t pt-4">
                        <h3 class="text-sm font-semibold">Credentials</h3>

                        <div class="space-y-1.5">
                            <Label for="api_key">
                                API key
                                <span class="text-muted-foreground ml-1 font-normal">
                                    (leave blank to keep current)
                                </span>
                            </Label>
                            <Input
                                id="api_key"
                                name="api_key"
                                type="password"
                                bind:value={apiKey}
                                placeholder="Enter a new API key to update"
                                autocomplete="new-password" />
                            <p class="text-muted-foreground text-xs">
                                Generate an API token in paperless-ngx under
                                <em>Settings → API</em>.
                            </p>
                        </div>
                    </div>
                </Card.Content>

                <Card.Footer class="flex justify-between">
                    <Button
                        type="button"
                        variant="destructive"
                        size="sm"
                        class="cursor-pointer"
                        onclick={() => (showRemoveDialog = true)}>
                        Remove source
                    </Button>

                    <Button
                        type="submit"
                        disabled={isSubmitting || !hasUnsavedChanges}
                        class="cursor-pointer">
                        {#if isSubmitting}
                            <Loader2 class="mr-2 h-4 w-4 animate-spin" />
                            Saving…
                        {:else}
                            Save changes
                        {/if}
                    </Button>
                </Card.Footer>
            </Card.Root>
        </form>
    </div>
</div>

<RemoveSourceDialog
    bind:open={showRemoveDialog}
    sourceId={data.source.id}
    sourceName={data.source.name} />
