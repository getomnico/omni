<script lang="ts">
    import * as HoverCard from '$lib/components/ui/hover-card'
    import { SourceType } from '$lib/types'
    import {
        getIconFromSearchResult,
        getSourceDisplayName,
        inferSourceFromUrl,
    } from '$lib/utils/icons'
    import { FileText } from '@lucide/svelte'

    type Props = {
        href: string
        title: string
        text: string
        snippet?: string
    }

    let { href, title, text, snippet }: Props = $props()

    // Remove markdown annotations, reduce consecutive whitespace to a single space, truncate to 80 chars
    function sanitizeCitedText(text: string) {
        // Remove markdown formatting
        let sanitized = text
            // Remove bold/italic markers
            .replace(/\*\*([^*]+)\*\*/g, '$1') // **bold**
            .replace(/\*([^*]+)\*/g, '$1') // *italic*
            .replace(/__([^_]+)__/g, '$1') // __bold__
            .replace(/_([^_]+)_/g, '$1') // _italic_
            // Remove links [text](url)
            .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
            // Remove inline code
            .replace(/`([^`]+)`/g, '$1')
            // Remove headers
            .replace(/^#+\s+/gm, '')
            // Replace multiple ellipses with single ellipsis
            .replace(/\.{2,}/g, '... ')
            // Reduce consecutive whitespace to single space
            .replace(/\s+/g, ' ')
            // Trim
            .trim()

        // Truncate to 80 chars with ellipsis
        if (sanitized.length > 80) {
            sanitized = sanitized.substring(0, 80) + '...'
        }

        return sanitized
    }
</script>

<HoverCard.Root>
    <HoverCard.Trigger
        {href}
        {title}
        target="_blank"
        rel="noreferrer noopener"
        class="text-muted-foreground hover:text-foreground/80 inline-block max-w-36 items-center 
        gap-1 truncate overflow-hidden border p-0.5 text-xs no-underline 
        transition-colors">
        [{text}]
    </HoverCard.Trigger>
    <HoverCard.Content>
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-1">
                {#if getIconFromSearchResult(href)}
                    <img
                        src={getIconFromSearchResult(href)}
                        alt=""
                        class="!m-0 h-4 w-4 flex-shrink-0" />
                {:else}
                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                {/if}
                <h4 class="text-muted-foreground text-xs font-semibold">
                    {getSourceDisplayName(inferSourceFromUrl(href) || SourceType.LOCAL_FILES)}
                </h4>
            </div>
            <h4 class="truncate overflow-hidden text-sm font-semibold">
                {title}
            </h4>
            <div class="text-muted-foreground overflow-hidden text-xs whitespace-break-spaces">
                {sanitizeCitedText(snippet || '')}
            </div>
        </div>
    </HoverCard.Content>
</HoverCard.Root>
