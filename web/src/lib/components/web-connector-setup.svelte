<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Textarea } from '$lib/components/ui/textarea'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import * as Collapsible from '$lib/components/ui/collapsible'
    import { ChevronDown } from '@lucide/svelte'
    import { toast } from 'svelte-sonner'

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
    let blacklistPatterns = $state('')
    let userAgent = $state('')
    let showAdvanced = $state(false)
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
            // Parse blacklist patterns (split by comma or newline, trim whitespace)
            const patterns = blacklistPatterns
                .split(/[,\n]/)
                .map((p) => p.trim())
                .filter((p) => p.length > 0)

            // Build config matching WebSourceConfig from connectors/web/src/config.rs
            const config = {
                root_url: rootUrl.trim(),
                max_depth: maxDepth,
                max_pages: maxPages,
                respect_robots_txt: respectRobotsTxt,
                include_subdomains: includeSubdomains,
                blacklist_patterns: patterns,
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
    <!-- Basic Configuration -->
    <div class="space-y-2">
        <Label for="root-url">Root URL *</Label>
        <Input
            id="root-url"
            bind:value={rootUrl}
            placeholder="https://docs.example.com"
            type="url"
            required />
        <p class="text-muted-foreground text-sm">
            The starting URL to crawl from. Must be a valid HTTP or HTTPS URL.
        </p>
    </div>

    <div class="grid grid-cols-2 gap-4">
        <div class="space-y-2">
            <Label for="max-depth">Max Crawl Depth</Label>
            <Input id="max-depth" bind:value={maxDepth} type="number" min="1" placeholder="10" />
            <p class="text-muted-foreground text-sm">How many links deep to follow</p>
        </div>

        <div class="space-y-2">
            <Label for="max-pages">Max Pages</Label>
            <Input id="max-pages" bind:value={maxPages} type="number" min="1" placeholder="10000" />
            <p class="text-muted-foreground text-sm">Maximum pages to crawl</p>
        </div>
    </div>

    <div class="flex items-center space-x-2">
        <Checkbox id="respect-robots" bind:checked={respectRobotsTxt} />
        <Label for="respect-robots" class="cursor-pointer font-normal">
            Respect robots.txt directives
        </Label>
    </div>

    <!-- Advanced Configuration (Collapsible) -->
    <Collapsible.Root bind:open={showAdvanced}>
        <Collapsible.Trigger class="flex items-center gap-2 text-sm font-medium">
            <ChevronDown
                class={`h-4 w-4 transition-transform ${showAdvanced ? 'rotate-180' : ''}`} />
            Advanced Options
        </Collapsible.Trigger>
        <Collapsible.Content class="mt-4 space-y-4">
            <div class="flex items-center space-x-2">
                <Checkbox id="include-subdomains" bind:checked={includeSubdomains} />
                <Label for="include-subdomains" class="cursor-pointer font-normal">
                    Include subdomains (e.g., crawl both docs.example.com and blog.example.com)
                </Label>
            </div>

            <div class="space-y-2">
                <Label for="blacklist-patterns">URL Blacklist Patterns</Label>
                <Textarea
                    id="blacklist-patterns"
                    bind:value={blacklistPatterns}
                    placeholder="/admin, /api, /login"
                    rows={3} />
                <p class="text-muted-foreground text-sm">
                    URL patterns to exclude (comma or newline separated)
                </p>
            </div>

            <div class="space-y-2">
                <Label for="user-agent">Custom User Agent (Optional)</Label>
                <Input
                    id="user-agent"
                    bind:value={userAgent}
                    placeholder="MyBot/1.0 (contact@example.com)" />
                <p class="text-muted-foreground text-sm">
                    Custom user agent string for crawler requests
                </p>
            </div>
        </Collapsible.Content>
    </Collapsible.Root>

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
