<script lang="ts">
    import { page } from '$app/stores'
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Search, FileText, Calendar, User, FilterIcon, Funnel } from '@lucide/svelte'
    import type { PageData } from './$types.js'
    import type { SearchResponse, SearchRequest } from '$lib/types/search.js'
    import AIAnswer from '$lib/components/AIAnswer.svelte'
    import { getDocumentIconPath, getSourceTypeFromId, getSourceIconPath } from '$lib/utils/icons'
    import { marked } from 'marked'

    let { data }: { data: PageData } = $props()

    // Create sources lookup map for efficient access
    let sourcesLookup = $derived(
        data.sources ? new Map(data.sources.map((s: any) => [s.id, s.sourceType])) : new Map(),
    )

    // inputQuery represents the current value in the search input
    let inputQuery = $state($page.url.searchParams.get('q') || '')
    // searchQuery represents the submitted query (from URL param)
    let searchQuery = $state($page.url.searchParams.get('q') || '')
    let isLoading = $state(false)

    // Get selected source types from server data (parsed from URL params)
    let selectedSourceTypes = $derived(new Set(data.selectedSourceTypes || []))

    const facetDisplayNames: Record<string, string> = {
        source_type: 'Source Type',
    }

    const sourceDisplayNames: Record<string, string> = {
        google_drive: 'Google Drive',
        gmail: 'Gmail',
        confluence: 'Confluence',
        jira: 'JIRA',
        slack: 'Slack',
        github: 'GitHub',
        local_files: 'Local Files',
    }

    let allFacets = $derived(data.searchResults?.facets || [])
    let sourceFacet = $derived(allFacets.find((f) => f.name === 'source_type'))
    let otherFacets = $derived(allFacets.filter((f) => f.name !== 'source_type'))

    function getDisplayValue(facetField: string, value: string): string {
        if (facetField === 'source_type') {
            return sourceDisplayNames[value] || value
        }
        return value
    }

    function toggleFilter(facetField: string, value: string) {
        const currentUrl = new URL(window.location.href)
        const currentValues = currentUrl.searchParams.getAll(facetField)

        if (currentValues.includes(value)) {
            // Remove this value
            currentUrl.searchParams.delete(facetField)
            currentValues
                .filter((v) => v !== value)
                .forEach((v) => {
                    currentUrl.searchParams.append(facetField, v)
                })
        } else {
            // Add this value
            currentUrl.searchParams.append(facetField, value)
        }

        // Navigate to the new URL
        window.location.href = currentUrl.toString()
    }

    function clearFilters() {
        const currentUrl = new URL(window.location.href)
        // Remove all filter params (for now just source_type)
        currentUrl.searchParams.delete('source_type')
        window.location.href = currentUrl.toString()
    }

    function clearFacetFilters(facetField: string) {
        const currentUrl = new URL(window.location.href)
        currentUrl.searchParams.delete(facetField)
        window.location.href = currentUrl.toString()
    }

    function getTotalSelectedFilters(): number {
        return selectedSourceTypes.size
    }

    function handleSearch() {
        if (inputQuery.trim()) {
            window.location.href = `/search?q=${encodeURIComponent(inputQuery.trim())}`
        }
    }

    function handleKeyPress(event: KeyboardEvent) {
        if (event.key === 'Enter') {
            handleSearch()
        }
    }

    function formatDate(dateStr: string) {
        return new Date(dateStr).toLocaleDateString()
    }

    function truncateContent(content: string, maxLength: number = 200) {
        if (content.length <= maxLength) return content
        return content.substring(0, maxLength) + '...'
    }

    function getSourceIcon(
        sourceId: string,
        contentType: string,
    ): { iconPath: string | null; useFileText: boolean } {
        console.log(`Looking up icon for source id ${sourceId}`)
        const sourceType = sourcesLookup.get(sourceId)
        if (!sourceType) {
            return { iconPath: null, useFileText: true }
        }

        const iconPath = getDocumentIconPath(sourceType, contentType)
        return { iconPath, useFileText: !iconPath }
    }

    function formatUrlAsBreadcrumbs(url: string): string {
        if (!url || url === 'No URL available') return url

        try {
            const urlObj = new URL(url)
            const domain = urlObj.hostname
            const pathParts = urlObj.pathname.split('/').filter((part) => part && part !== '')

            if (pathParts.length === 0) {
                return domain
            }

            return [domain, ...pathParts].join(' › ')
        } catch {
            return url
        }
    }

    function renderHighlight(text: string): string {
        // Parse as inline markdown to avoid wrapping in <p> tags
        return marked.parseInline(text)
    }
</script>

<svelte:head>
    <title>Search Results - Clio</title>
</svelte:head>

<div>
    <!-- Search Header -->
    <div class="mb-8">
        <div class="mb-4 flex items-center gap-4">
            <div
                class="flex flex-1 items-center rounded-full border border-gray-300 bg-white p-2 shadow-sm"
            >
                <div class="w-1"></div>
                <Input
                    type="text"
                    bind:value={inputQuery}
                    placeholder="Search across your organization..."
                    class="flex-1 border-none bg-transparent shadow-none focus:ring-0 focus-visible:ring-0"
                    onkeypress={handleKeyPress}
                />
                <Button size="icon" variant="link" onclick={handleSearch} disabled={isLoading}>
                    <Search class="h-6 w-6" />
                </Button>
            </div>
        </div>

        {#if data.searchResults}
            <div class="px-6 text-sm text-gray-600">
                Found {data.searchResults.total_count} results in {data.searchResults
                    .query_time_ms}ms for "{data.searchResults.query}"
                {#if getTotalSelectedFilters() > 0}
                    <span class="ml-2"
                        >• {getTotalSelectedFilters()} filter{getTotalSelectedFilters() > 1
                            ? 's'
                            : ''} applied</span
                    >
                {/if}
            </div>
        {/if}
    </div>

    <!-- Other Facets (above search results) -->
    {#if data.searchResults && otherFacets.length > 0}
        <div class="mb-6">
            <div class="flex flex-wrap gap-4">
                {#each otherFacets as facet}
                    <div class="min-w-48 rounded-lg border bg-white p-4">
                        <div class="mb-3 flex items-center justify-between">
                            <h3 class="text-sm font-medium text-gray-900">
                                {facetDisplayNames[facet.name] || facet.name}
                            </h3>
                            {#if facet.name === 'source_type' && selectedSourceTypes.size > 0}
                                <Button
                                    variant="ghost"
                                    size="sm"
                                    onclick={() => clearFacetFilters(facet.name)}
                                    class="h-6 px-2 text-xs"
                                >
                                    Clear
                                </Button>
                            {/if}
                        </div>
                        <div class="max-h-32 space-y-2 overflow-y-auto">
                            {#each facet.values.slice(0, 5) as facetValue}
                                <label
                                    class="flex cursor-pointer items-center justify-between rounded p-1 text-xs hover:bg-gray-50"
                                >
                                    <div class="flex items-center gap-2">
                                        <input
                                            type="checkbox"
                                            checked={facet.name === 'source_type' &&
                                                selectedSourceTypes.has(facetValue.value)}
                                            onchange={() =>
                                                toggleFilter(facet.name, facetValue.value)}
                                            class="h-3 w-3 rounded border-gray-300 text-blue-600"
                                        />
                                        <span class="truncate text-gray-700">
                                            {getDisplayValue(facet.name, facetValue.value)}
                                        </span>
                                    </div>
                                    <span
                                        class="ml-2 rounded bg-gray-100 px-1 py-0.5 text-xs text-gray-500"
                                    >
                                        {facetValue.count}
                                    </span>
                                </label>
                            {/each}
                            {#if facet.values.length > 5}
                                <div class="pt-1 text-center text-xs text-gray-500">
                                    +{facet.values.length - 5} more
                                </div>
                            {/if}
                        </div>
                    </div>
                {/each}
            </div>
        </div>
    {/if}

    <!-- AI Answer Section -->
    {#if data.searchResults && searchQuery.trim() && data.aiAnswerEnabled}
        <AIAnswer
            searchRequest={{
                query: searchQuery,
                limit: 20,
                offset: 0,
                mode: 'hybrid',
            }}
        />
    {/if}

    <div class="flex gap-6 px-6">
        <!-- Search Results -->
        <div class="flex-1">
            {#if data.searchResults}
                {#if data.searchResults.results.length > 0}
                    <div class="space-y-8">
                        {#each data.searchResults.results as result}
                            {@const srcIcon = getSourceIcon(
                                result.document.source_id,
                                result.document.content_type,
                            )}
                            <div class="flex gap-3">
                                <!-- Icon -->
                                <div class="flex-shrink-0">
                                    {#if srcIcon.useFileText}
                                        <FileText class="h-5 w-5 text-gray-400" />
                                    {:else}
                                        <img
                                            src={srcIcon.iconPath}
                                            alt="Source icon"
                                            class="h-7 w-7"
                                        />
                                    {/if}
                                </div>

                                <!-- Content -->
                                <div class="min-w-0 flex-1">
                                    <!-- Title + URL as single link -->
                                    <a
                                        href={result.document.url || '#'}
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        class="group block"
                                    >
                                        <h3
                                            class="text-xl leading-tight text-blue-700 group-hover:underline"
                                        >
                                            {result.document.title}
                                        </h3>
                                        {#if result.document.url}
                                            <div class="text-muted-foreground mb-1 text-sm">
                                                {formatUrlAsBreadcrumbs(
                                                    result.document.url || 'No URL available',
                                                )}
                                            </div>
                                        {/if}
                                    </a>

                                    <!-- Date -->
                                    <div class="mb-2 text-sm text-gray-500">
                                        {formatDate(result.document.created_at)}
                                    </div>

                                    <!-- Excerpt/Content -->
                                    {#if result.highlights.length > 0}
                                        <div
                                            class="highlight-content text-sm leading-relaxed text-gray-600"
                                        >
                                            {#each result.highlights.slice(0, 2) as highlight}
                                                <span>{@html renderHighlight(highlight)}</span>
                                                {#if highlight !== result.highlights[result.highlights.length - 1]}
                                                    <span> ... </span>
                                                {/if}
                                            {/each}
                                        </div>
                                    {:else if result.content}
                                        <div class="text-sm leading-relaxed text-gray-600">
                                            {truncateContent(result.content)}
                                        </div>
                                    {/if}
                                </div>
                            </div>
                        {/each}
                    </div>

                    <!-- Pagination -->
                    {#if data.searchResults?.has_more}
                        <div class="mt-8 text-center">
                            <Button variant="outline">Load More Results</Button>
                        </div>
                    {/if}
                {:else}
                    <div class="py-12 text-center">
                        <Search class="mx-auto mb-4 h-12 w-12 text-gray-400" />
                        <h3 class="mb-2 text-lg font-medium text-gray-900">No results found</h3>
                        <p class="mb-4 text-gray-600">
                            {#if getTotalSelectedFilters() > 0}
                                No results found with the current filters. Try clearing filters or
                                adjusting your search.
                            {:else}
                                Try adjusting your search terms or check if your data sources are
                                connected and indexed.
                            {/if}
                        </p>
                        {#if getTotalSelectedFilters() > 0}
                            <Button variant="outline" onclick={clearFilters} class="mr-2">
                                Clear Filters
                            </Button>
                        {/if}
                        <Button variant="outline" onclick={() => (window.location.href = '/')}>
                            Back to Home
                        </Button>
                    </div>
                {/if}
            {:else if $page.url.searchParams.get('q')}
                <div class="py-12 text-center">
                    <div
                        class="mx-auto mb-4 h-8 w-8 animate-spin rounded-full border-4 border-gray-300 border-t-blue-600"
                    ></div>
                    <p class="text-gray-600">Searching...</p>
                </div>
            {:else}
                <div class="py-12 text-center">
                    <Search class="mx-auto mb-4 h-12 w-12 text-gray-400" />
                    <h3 class="mb-2 text-lg font-medium text-gray-900">Enter a search query</h3>
                    <p class="text-gray-600">
                        Search across your organization's documents, emails, and more.
                    </p>
                </div>
            {/if}
        </div>

        <!-- Source Facets Sidebar -->
        {#if data.searchResults && sourceFacet}
            <div class="w-80">
                <div class="">
                    <div class="mb-4 flex items-center justify-between">
                        <h3 class="flex flex-1 items-center gap-2 px-4 text-base font-semibold">
                            <div class="flex-1">Filter by Source</div>
                            <Funnel class="h-4 w-4" />
                        </h3>
                        {#if selectedSourceTypes.size > 0}
                            <Button
                                variant="ghost"
                                size="sm"
                                onclick={() => clearFacetFilters('source_type')}
                                class="text-xs"
                            >
                                Clear
                            </Button>
                        {/if}
                    </div>
                    <div class="flex flex-col space-y-2">
                        {#each sourceFacet.values as facetValue}
                            {@const sourceIcon = getSourceIconPath(facetValue.value)}
                            {@const isSelected = selectedSourceTypes.has(facetValue.value)}
                            <Button
                                variant="ghost"
                                class="flex cursor-pointer justify-between rounded-full {isSelected
                                    ? 'bg-blue-50 hover:bg-blue-100'
                                    : 'hover:bg-gray-50'}"
                                onclick={() => toggleFilter('source_type', facetValue.value)}
                            >
                                <div class="flex items-center gap-2">
                                    {#if sourceIcon}
                                        <img
                                            src={sourceIcon}
                                            alt="{facetValue.value} icon"
                                            class="h-4 w-4"
                                        />
                                    {:else}
                                        <FileText class="h-4 w-4 text-gray-400" />
                                    {/if}
                                    <span
                                        class="text-sm font-medium {isSelected
                                            ? 'text-blue-700'
                                            : 'text-gray-700'}"
                                    >
                                        {getDisplayValue('source_type', facetValue.value)}
                                    </span>
                                </div>
                                <span
                                    class="rounded-full px-2 py-0.5 text-xs {isSelected
                                        ? 'bg-blue-100 text-blue-700'
                                        : 'bg-gray-100 text-gray-500'}"
                                >
                                    {facetValue.count}
                                </span>
                            </Button>
                        {/each}
                    </div>
                </div>

                {#if getTotalSelectedFilters() > 0}
                    <div class="mt-4">
                        <Button variant="outline" size="sm" onclick={clearFilters} class="w-full">
                            Clear All Filters
                        </Button>
                    </div>
                {/if}
            </div>
        {/if}
    </div>
</div>

<style>
    :global(.highlight-content strong) {
        font-weight: 600;
        color: rgb(17 24 39);
    }
</style>
