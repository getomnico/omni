<script lang="ts">
    import { SourceType } from '$lib/types'
    import type { SearchResult } from '$lib/types/search'
    import { getDocumentIconPath } from '$lib/utils/icons'
    import { FileText } from '@lucide/svelte'
    import { marked } from 'marked'
    import GmailMetadata from './gmail-metadata.svelte'
    import SlackMetadata from './slack-metadata.svelte'
    import JiraMetadata from './jira-metadata.svelte'
    import ImapCitationSource from './imap-citation-source.svelte'

    let { result, sourcesLookup }: { result: SearchResult; sourcesLookup: Map<string, string> } =
        $props()

    let sourceType = $derived(sourcesLookup.get(result.document.source_id))
    let extra = $derived(result.document.metadata?.extra || {})
    let attributes = $derived(result.document.attributes || {})
    let metadata = $derived(result.document.metadata)

    let icon = $derived.by(() => {
        if (!sourceType) return { iconPath: null, useFileText: true }
        const iconPath = getDocumentIconPath(sourceType, result.document.content_type)
        return { iconPath, useFileText: !iconPath }
    })

    function formatDate(dateStr: string) {
        const d = new Date(dateStr)
        const day = d.getDate()
        const month = d.toLocaleString('en-US', { month: 'short' })
        const year = d.getFullYear()
        return `${day} ${month} ${year}`
    }

    function getDisplayDate(): string {
        const metadataDate = metadata?.updated_at || metadata?.created_at
        return formatDate(metadataDate || result.document.updated_at)
    }

    function truncateContent(content: string, maxLength: number = 200) {
        if (content.length <= maxLength) return content
        return content.substring(0, maxLength) + '...'
    }

    function formatUrlAsBreadcrumbs(url: string, maxLength: number = 120): string {
        if (!url || url === 'No URL available') return url

        try {
            const urlObj = new URL(url)
            const domain = urlObj.hostname
            const pathParts = urlObj.pathname.split('/').filter((part) => part && part !== '')

            if (pathParts.length === 0) {
                return domain
            }

            const breadcrumb = [domain, ...pathParts].join(' › ')

            if (breadcrumb.length <= maxLength) {
                return breadcrumb
            }

            const separator = ' › '
            const ellipsis = '…'

            const domainLength = domain.length + separator.length
            const ellipsisLength = ellipsis.length + separator.length * 2
            let budget = maxLength - domainLength

            let i = 0
            let j = pathParts.length

            for (let removeCount = 0; removeCount <= pathParts.length; removeCount++) {
                for (let start = 0; start <= pathParts.length - removeCount; start++) {
                    const end = start + removeCount

                    let totalLength = 0

                    for (let k = 0; k < start; k++) {
                        totalLength += pathParts[k].length + separator.length
                    }

                    for (let k = end; k < pathParts.length; k++) {
                        totalLength += pathParts[k].length + separator.length
                    }

                    if (removeCount > 0 && start < end) {
                        totalLength += ellipsisLength
                    }

                    if (totalLength <= budget) {
                        i = start
                        j = end
                        break
                    }
                }

                if (j !== pathParts.length || i !== 0) {
                    break
                }
            }

            if (i === 0 && j === pathParts.length) {
                return breadcrumb
            } else if (i === j) {
                return breadcrumb
            } else {
                const firstParts = pathParts.slice(0, i)
                const lastParts = pathParts.slice(j)
                const parts = [domain, ...firstParts, ellipsis, ...lastParts]
                return parts.join(separator)
            }
        } catch {
            return url
        }
    }

    function renderHighlight(text: string): string {
        return marked.parseInline(text.replaceAll('\n', ' '))
    }
</script>

<div class="flex gap-3">
    <!-- Icon -->
    <div class="flex-shrink-0">
        {#if icon.useFileText}
            <FileText class="h-5 w-5 text-gray-400" />
        {:else}
            <img src={icon.iconPath} alt="Source icon" class="h-7 w-7" />
        {/if}
    </div>

    <!-- Content -->
    <div class="min-w-0 flex-1">
        <!-- Title + URL -->
        <a
            href={result.document.url || '#'}
            target="_blank"
            rel="noopener noreferrer"
            class="group block">
            <h3 class="text-xl leading-tight text-blue-700 group-hover:underline">
                {result.document.title}
            </h3>
            {#if result.document.url}
                <div class="text-muted-foreground mb-1 text-sm">
                    {formatUrlAsBreadcrumbs(result.document.url || 'No URL available')}
                </div>
            {/if}
        </a>

        <!-- Source-specific metadata -->
        {#if sourceType === SourceType.GMAIL}
            <GmailMetadata {extra} />
        {:else if sourceType === SourceType.SLACK}
            <SlackMetadata {extra} {metadata} />
        {:else if sourceType === SourceType.JIRA}
            <JiraMetadata {attributes} />
        {:else if sourceType === SourceType.IMAP}
            <ImapCitationSource source={attributes.doc_source} />
        {:else if sourceType === SourceType.NEXTCLOUD && attributes.path}
            <p class="text-muted-foreground/70 mb-1 text-xs">{attributes.path}</p>
        {/if}

        <!-- Date + Excerpt/Content -->
        {#if result.highlights.length > 0}
            <div class="highlight-content line-clamp-3 text-sm leading-relaxed text-gray-600">
                <span class="text-gray-500">{getDisplayDate()}</span>
                <span class="text-gray-400"> · </span>
                {#each result.highlights.slice(0, 2) as highlight}
                    <span>{@html renderHighlight(highlight)}</span>
                    {#if highlight !== result.highlights[result.highlights.length - 1]}
                        <span> ... </span>
                    {/if}
                {/each}
            </div>
        {:else if result.content}
            <div class="text-sm leading-relaxed text-gray-600">
                <span class="text-gray-500">{getDisplayDate()}</span>
                <span class="text-gray-400"> · </span>
                {truncateContent(result.content)}
            </div>
        {:else}
            <div class="text-sm text-gray-500">
                {getDisplayDate()}
            </div>
        {/if}
    </div>
</div>
