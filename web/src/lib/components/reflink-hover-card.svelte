<script lang="ts">
    import * as HoverCard from '$lib/components/ui/hover-card'
    import { getIconFromSearchResult } from '$lib/utils/icons'
    import { FileText, Globe } from '@lucide/svelte'

    type Props = {
        href: string | null
        title: string
        snippet?: string
        /** Raw URL string for icon inference (may be null for documents). */
        iconHint?: string | null
        /** Connector display name (e.g. "Files", "Web", "Gmail"). */
        sourceName: string
        /** Location label (e.g. "Pages 3–5", "Document excerpt"). */
        locationLabel?: string | null
    }

    let {
        href,
        title,
        snippet,
        iconHint = null,
        sourceName,
        locationLabel = null,
    }: Props = $props()

    // Remove markdown annotations, reduce consecutive whitespace to a single space, truncate to 80 chars
    const citationChipClass =
        'text-muted-foreground hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50 focus-visible:border-ring focus-visible:ring-ring/50 ml-0.5 inline-flex h-5 max-w-40 cursor-pointer items-center gap-1 overflow-hidden rounded-md border border-border/60 bg-transparent px-1.5 py-0 text-xs font-medium no-underline outline-none transition-colors focus-visible:ring-[3px]'

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
    <HoverCard.Content>
        <div class="flex flex-col gap-1">
            <div class="flex items-center gap-1">
                {#if iconHint && getIconFromSearchResult(iconHint)}
                    <img
                        src={getIconFromSearchResult(iconHint)}
                        alt=""
                        class="!m-0 h-4 w-4 flex-shrink-0" />
                {:else if sourceName === 'Web'}
                    <Globe class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                {:else}
                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                {/if}
                <h4 class="text-muted-foreground text-xs font-semibold">
                    {sourceName}
                </h4>
            </div>
            <h4 class="truncate overflow-hidden text-sm font-semibold">
                {title}
            </h4>
            {#if locationLabel}
                <p class="text-muted-foreground/70 text-xs">
                    {locationLabel}
                </p>
            {/if}
            <div class="text-muted-foreground overflow-hidden text-xs whitespace-break-spaces">
                {sanitizeCitedText(snippet || '')}
            </div>
        </div>
    </HoverCard.Content>
    <!-- Inline chip with icon, truncated title, and existing hover card.
         The chip replaces the numeric marker while preserving punctuation adjacency.
         For HTTP(S) sources, renders as an anchor with target=_blank.
         For non-navigable sources, renders as a button with type=button. -->
    {@const isNavigable =
        href !== null && (href.startsWith('http://') || href.startsWith('https://'))}
    <HoverCard.Trigger {title}>
        {#snippet child({ props })}
            {#if isNavigable}
                <a
                    {...props}
                    role={undefined}
                    {href}
                    target="_blank"
                    rel="noreferrer noopener"
                    class={citationChipClass}>
                    {#if iconHint && getIconFromSearchResult(iconHint)}
                        <img
                            src={getIconFromSearchResult(iconHint)}
                            alt=""
                            class="!m-0 h-3 w-3 flex-shrink-0" />
                    {:else if sourceName === 'Web'}
                        <Globe class="text-muted-foreground h-3 w-3 flex-shrink-0" />
                    {:else}
                        <FileText class="text-muted-foreground h-3 w-3 flex-shrink-0" />
                    {/if}
                    <span class="truncate">{title}</span>
                </a>
            {:else}
                <button {...props} type="button" class={citationChipClass}>
                    {#if iconHint && getIconFromSearchResult(iconHint)}
                        <img
                            src={getIconFromSearchResult(iconHint)}
                            alt=""
                            class="!m-0 h-3 w-3 flex-shrink-0" />
                    {:else if sourceName === 'Web'}
                        <Globe class="text-muted-foreground h-3 w-3 flex-shrink-0" />
                    {:else}
                        <FileText class="text-muted-foreground h-3 w-3 flex-shrink-0" />
                    {/if}
                    <span class="truncate">{title}</span>
                </button>
            {/if}
        {/snippet}
    </HoverCard.Trigger>
</HoverCard.Root>
