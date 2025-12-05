<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import { Input } from '$lib/components/ui/input'
    import { Label } from '$lib/components/ui/label'
    import { Checkbox } from '$lib/components/ui/checkbox'
    import { X } from '@lucide/svelte'
    import type { WebSourceConfig } from '$lib/types'

    interface Props {
        rootUrl?: string
        maxDepth?: number
        maxPages?: number
        respectRobotsTxt?: boolean
        includeSubdomains?: boolean
        blacklistPatterns?: string[]
        userAgent?: string
        disabled?: boolean
    }

    let {
        rootUrl = $bindable(''),
        maxDepth = $bindable(10),
        maxPages = $bindable(10000),
        respectRobotsTxt = $bindable(true),
        includeSubdomains = $bindable(false),
        blacklistPatterns = $bindable([]),
        userAgent = $bindable(''),
        disabled = false,
    }: Props = $props()

    let blacklistInput = $state('')

    function addBlacklistPattern() {
        const pattern = blacklistInput.trim()
        if (pattern && !blacklistPatterns.includes(pattern)) {
            blacklistPatterns = [...blacklistPatterns, pattern]
            blacklistInput = ''
        }
    }

    function removeBlacklistPattern(pattern: string) {
        blacklistPatterns = blacklistPatterns.filter((p) => p !== pattern)
    }
</script>

<div class="space-y-6">
    <!-- Root URL -->
    <div class="space-y-2">
        <Label for="rootUrl">Root URL *</Label>
        <Input
            id="rootUrl"
            name="rootUrl"
            type="url"
            bind:value={rootUrl}
            placeholder="https://docs.example.com"
            {disabled}
            required />
        <p class="text-muted-foreground text-sm">
            The starting URL to crawl from. Must be a valid HTTP or HTTPS URL.
        </p>
    </div>

    <!-- Max Depth and Max Pages -->
    <div class="grid grid-cols-2 gap-4">
        <div class="space-y-2">
            <Label for="maxDepth">Max Crawl Depth</Label>
            <Input
                id="maxDepth"
                name="maxDepth"
                type="number"
                min="1"
                bind:value={maxDepth}
                {disabled}
                placeholder="10" />
            <p class="text-muted-foreground text-sm">How many links deep to follow</p>
        </div>

        <div class="space-y-2">
            <Label for="maxPages">Max Pages</Label>
            <Input
                id="maxPages"
                name="maxPages"
                type="number"
                min="1"
                bind:value={maxPages}
                {disabled}
                placeholder="10000" />
            <p class="text-muted-foreground text-sm">Maximum pages to crawl</p>
        </div>
    </div>

    <!-- Checkboxes -->
    <div class="space-y-3">
        <div class="flex items-center space-x-2">
            <Checkbox
                id="respectRobotsTxt"
                name="respectRobotsTxt"
                bind:checked={respectRobotsTxt}
                {disabled} />
            <Label for="respectRobotsTxt" class="cursor-pointer font-normal">
                Respect robots.txt directives
            </Label>
        </div>

        <div class="flex items-center space-x-2">
            <Checkbox
                id="includeSubdomains"
                name="includeSubdomains"
                bind:checked={includeSubdomains}
                {disabled} />
            <Label for="includeSubdomains" class="cursor-pointer font-normal">
                Include subdomains (e.g., crawl both docs.example.com and blog.example.com)
            </Label>
        </div>
    </div>

    <!-- User Agent -->
    <div class="space-y-2">
        <Label for="userAgent">Custom User Agent (Optional)</Label>
        <Input
            id="userAgent"
            name="userAgent"
            bind:value={userAgent}
            placeholder="MyBot/1.0 (contact@example.com)"
            {disabled} />
        <p class="text-muted-foreground text-sm">Custom user agent string for crawler requests</p>
    </div>

    <!-- Blacklist Patterns -->
    <div class="space-y-3">
        <Label>URL Blacklist Patterns</Label>
        <p class="text-muted-foreground text-sm">
            URL patterns to exclude (comma or newline separated)
        </p>

        <div class="flex gap-2">
            <Input
                bind:value={blacklistInput}
                placeholder="/admin, /api, /login"
                {disabled}
                class="flex-1"
                onkeydown={(e) => {
                    if (e.key === 'Enter') {
                        e.preventDefault()
                        addBlacklistPattern()
                    }
                }} />
            <Button
                type="button"
                variant="secondary"
                onclick={addBlacklistPattern}
                disabled={disabled || !blacklistInput.trim()}>
                Add
            </Button>
        </div>

        {#if blacklistPatterns.length > 0}
            <div class="flex flex-wrap gap-2">
                {#each blacklistPatterns as pattern}
                    <div
                        class="bg-secondary text-secondary-foreground hover:bg-secondary/80 inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium transition-colors">
                        <span>{pattern}</span>
                        <button
                            type="button"
                            onclick={() => removeBlacklistPattern(pattern)}
                            class="hover:bg-secondary-foreground/20 ml-1 rounded-full p-0.5 transition-colors"
                            aria-label="Remove {pattern}">
                            <X class="h-3 w-3" />
                        </button>
                    </div>
                {/each}
            </div>
        {/if}
    </div>

    <!-- Hidden inputs for form submission -->
    {#each blacklistPatterns as pattern}
        <input type="hidden" name="blacklistPatterns" value={pattern} />
    {/each}
</div>
