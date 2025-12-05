<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { toast } from 'svelte-sonner'
    import type { WebSourceConfig } from '$lib/types'
    import WebConnectorForm from '$lib/components/web-connector-form.svelte'

    interface Props {
        onSuccess?: () => void
        onCancel?: () => void
    }

    let { onSuccess, onCancel }: Props = $props()

    let rootUrl = $state('')
    let maxDepth = $state(10)
    let maxPages = $state(10000)
    let respectRobotsTxt = $state(true)
    let includeSubdomains = $state(false)
    let blacklistPatterns = $state<string[]>([])
    let userAgent = $state('')
    let isSubmitting = $state(false)

    function validateUrl(url: string): boolean {
        try {
            const parsed = new URL(url)
            return parsed.protocol === 'http:' || parsed.protocol === 'https:'
        } catch {
            return false
        }
    }

    async function handleSubmit() {
        if (!rootUrl.trim()) {
            toast.error('Root URL is required')
            return
        }

        if (!validateUrl(rootUrl)) {
            toast.error('Please enter a valid HTTP or HTTPS URL')
            return
        }

        if (maxDepth < 1) {
            toast.error('Max depth must be at least 1')
            return
        }

        if (maxPages < 1) {
            toast.error('Max pages must be at least 1')
            return
        }

        isSubmitting = true
        try {
            const config: WebSourceConfig = {
                root_url: rootUrl.trim(),
                max_depth: maxDepth,
                max_pages: maxPages,
                respect_robots_txt: respectRobotsTxt,
                include_subdomains: includeSubdomains,
                blacklist_patterns: blacklistPatterns,
                user_agent: userAgent.trim() || null,
            }

            // Create source with config (no credentials needed for web connector)
            const sourceResponse = await fetch('/api/sources', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name: `Web: ${new URL(rootUrl).hostname}`,
                    sourceType: 'web',
                    config: config,
                }),
            })

            if (!sourceResponse.ok) {
                const error = await sourceResponse.text()
                throw new Error(error || 'Failed to create web source')
            }

            const source = await sourceResponse.json()

            // Trigger initial sync
            const syncResponse = await fetch(`/api/sources/${source.id}/sync`, {
                method: 'POST',
            })

            if (!syncResponse.ok) {
                console.warn('Failed to start initial sync, but source was created')
            }

            toast.success('Web source connected successfully!')
            onSuccess?.()
        } catch (error: any) {
            console.error('Error setting up web connector:', error)
            toast.error(error.message || 'Failed to set up web connector')
        } finally {
            isSubmitting = false
        }
    }
</script>

<div class="space-y-4">
    <WebConnectorForm
        bind:rootUrl
        bind:maxDepth
        bind:maxPages
        bind:respectRobotsTxt
        bind:includeSubdomains
        bind:blacklistPatterns
        bind:userAgent
        disabled={isSubmitting} />

    <!-- Actions -->
    <div class="flex justify-end gap-2 pt-4">
        <Button
            variant="outline"
            onclick={() => onCancel?.()}
            disabled={isSubmitting}
            class="cursor-pointer">
            Cancel
        </Button>
        <Button onclick={handleSubmit} disabled={isSubmitting} class="cursor-pointer">
            {isSubmitting ? 'Connecting...' : 'Connect'}
        </Button>
    </div>
</div>
