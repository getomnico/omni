<script lang="ts">
    import { enhance } from '$app/forms'
    import { Button } from '$lib/components/ui/button'
    import { Label } from '$lib/components/ui/label'
    import { Switch } from '$lib/components/ui/switch'
    import * as Card from '$lib/components/ui/card'
    import * as Alert from '$lib/components/ui/alert'
    import { Input } from '$lib/components/ui/input'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { AlertCircle, Loader2, Globe } from '@lucide/svelte'
    import { onMount } from 'svelte'
    import { beforeNavigate } from '$app/navigation'
    import type { PageProps } from './$types'
    import type { WebSourceConfig } from '$lib/types'
    import WebConnectorForm from '$lib/components/web-connector-form.svelte'

    let { data }: PageProps = $props()

    const getConfig = (): Partial<WebSourceConfig> => {
        if (!data.webSource?.config) return {}
        return typeof data.webSource.config === 'string'
            ? JSON.parse(data.webSource.config)
            : data.webSource.config
    }

    const config = getConfig()

    let webEnabled = $state(data.webSource ? data.webSource.isActive : false)
    let rootUrl = $state(config.root_url || '')
    let maxDepth = $state(config.max_depth ?? 10)
    let maxPages = $state(config.max_pages ?? 10000)
    let respectRobotsTxt = $state(config.respect_robots_txt ?? true)
    let includeSubdomains = $state(config.include_subdomains ?? false)
    let blacklistPatterns = $state<string[]>(
        config.blacklist_patterns && Array.isArray(config.blacklist_patterns)
            ? config.blacklist_patterns
            : [],
    )
    let userAgent = $state(config.user_agent || '')
    let isSubmitting = $state(false)
    let formErrors = $state<string[]>([])
    let hasUnsavedChanges = $state(false)
    let skipUnsavedCheck = $state(false)

    let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

    let originalWebEnabled = data.webSource ? data.webSource.isActive : false
    let originalRootUrl = rootUrl
    let originalMaxDepth = maxDepth
    let originalMaxPages = maxPages
    let originalRespectRobotsTxt = respectRobotsTxt
    let originalIncludeSubdomains = includeSubdomains
    let originalBlacklistPatterns: string[] = [...blacklistPatterns]
    let originalUserAgent = userAgent

    function validateForm() {
        formErrors = []

        if (webEnabled && !rootUrl.trim()) {
            formErrors = [...formErrors, 'Root URL is required when web crawler is enabled']
            return false
        }

        if (webEnabled && rootUrl.trim()) {
            try {
                new URL(rootUrl.trim())
            } catch {
                formErrors = [...formErrors, 'Invalid root URL']
                return false
            }
        }

        if (maxDepth < 1) {
            formErrors = [...formErrors, 'Max depth must be at least 1']
            return false
        }

        if (maxPages < 1) {
            formErrors = [...formErrors, 'Max pages must be at least 1']
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
        const blacklistChanged =
            JSON.stringify(blacklistPatterns.sort()) !==
            JSON.stringify(originalBlacklistPatterns.sort())

        hasUnsavedChanges =
            webEnabled !== originalWebEnabled ||
            rootUrl !== originalRootUrl ||
            maxDepth !== originalMaxDepth ||
            maxPages !== originalMaxPages ||
            respectRobotsTxt !== originalRespectRobotsTxt ||
            includeSubdomains !== originalIncludeSubdomains ||
            userAgent !== originalUserAgent ||
            blacklistChanged
    })
</script>

<div class="h-full overflow-y-auto p-6 py-8 pb-24">
    <div class="mx-auto max-w-screen-lg space-y-8">
        <div>
            <h1 class="text-3xl font-bold tracking-tight">Configure Web Crawler</h1>
            <p class="text-muted-foreground mt-2">
                Configure web crawler to index content from public websites
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
                                <Globe class="h-5 w-5" />
                                Web Crawler
                            </Card.Title>
                            <Card.Description class="mt-1">
                                Index content from public websites and documentation
                            </Card.Description>
                        </div>
                        <div class="flex items-center gap-2">
                            <Label for="webEnabled" class="text-sm">Enabled</Label>
                            <Switch
                                id="webEnabled"
                                bind:checked={webEnabled}
                                name="webEnabled"
                                class="cursor-pointer" />
                        </div>
                    </div>
                </Card.Header>

                <Card.Content>
                    <WebConnectorForm
                        bind:rootUrl
                        bind:maxDepth
                        bind:maxPages
                        bind:respectRobotsTxt
                        bind:includeSubdomains
                        bind:blacklistPatterns
                        bind:userAgent
                        disabled={!webEnabled} />
                </Card.Content>
            </Card.Root>

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
