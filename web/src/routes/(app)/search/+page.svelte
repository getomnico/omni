<script lang="ts">
    import { page } from '$app/stores'
    import AIAnswer from '$lib/components/ai-answer.svelte'
    import SearchResultItem from '$lib/components/search-results/search-result-item.svelte'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import * as Pagination from '$lib/components/ui/pagination/index.js'
    import { getSourceIconPath } from '$lib/utils/icons'
    import { FileText, Funnel, Search } from '@lucide/svelte'
    import type { PageData } from './$types.js'

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

    // Active source types from both URL params and in-query operators (e.g. in:drive)
    let activeFilterFacets = $derived(data.searchResults?.active_filters || [])
    let activeSourceTypes = $derived(
        new Set([
            ...(data.selectedSourceTypes || []),
            ...(activeFilterFacets
                .find((f) => f.name === 'source_type')
                ?.values.map((v) => v.value) || []),
        ]),
    )
    let hasActiveFilters = $derived(activeFilterFacets.length > 0)

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
        local_files: 'Files',
        web: 'Web',
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

        // Reset to page 1 when filters change
        currentUrl.searchParams.delete('page')
        window.location.href = currentUrl.toString()
    }

    function clearFilters() {
        const currentUrl = new URL(window.location.href)
        currentUrl.searchParams.delete('source_type')
        currentUrl.searchParams.delete('page')
        window.location.href = currentUrl.toString()
    }

    function clearFacetFilters(facetField: string) {
        const currentUrl = new URL(window.location.href)
        currentUrl.searchParams.delete(facetField)
        currentUrl.searchParams.delete('page')
        window.location.href = currentUrl.toString()
    }

    function getTotalSelectedFilters(): number {
        return activeSourceTypes.size
    }

    let totalPages = $derived(
        data.searchResults ? Math.ceil(data.searchResults.total_count / data.pageSize) : 1,
    )

    function navigateToPage(newPage: number) {
        const currentUrl = new URL(window.location.href)
        if (newPage <= 1) {
            currentUrl.searchParams.delete('page')
        } else {
            currentUrl.searchParams.set('page', String(newPage))
        }
        window.location.href = currentUrl.toString()
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
</script>

<svelte:head>
    <title>Search Results - Omni</title>
</svelte:head>

<div class="mt-4 px-8 pb-24">
    <!-- Search Header -->
    <div class="mb-8">
        <div class="mb-4 flex items-center gap-4">
            <div
                class="flex flex-1 items-center rounded-full border border-gray-300 bg-white p-2 shadow-sm">
                <div class="w-1"></div>
                <Input
                    type="text"
                    bind:value={inputQuery}
                    placeholder="Search across your organization..."
                    class="flex-1 border-none bg-transparent shadow-none focus:ring-0 focus-visible:ring-0"
                    onkeypress={handleKeyPress} />
                <Button size="icon" variant="link" onclick={handleSearch} disabled={isLoading}>
                    <Search class="h-6 w-6" />
                </Button>
            </div>
        </div>

        {#if data.searchResults}
            <div class="px-6 text-sm text-gray-600">
                Found {data.searchResults.total_count} results in
                {data.searchResults.query_time_ms}ms for "{data.searchResults.query}"
                {#if getTotalSelectedFilters() > 0}
                    <span
                        >• {getTotalSelectedFilters()} filter{getTotalSelectedFilters() > 1
                            ? 's'
                            : ''} applied</span>
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
                                    class="h-6 cursor-pointer px-2 text-xs">
                                    Clear
                                </Button>
                            {/if}
                        </div>
                        <div class="max-h-32 space-y-2 overflow-y-auto">
                            {#each facet.values.slice(0, 5) as facetValue}
                                <label
                                    class="flex cursor-pointer items-center justify-between rounded p-1 text-xs hover:bg-gray-50">
                                    <div class="flex items-center gap-2">
                                        <input
                                            type="checkbox"
                                            checked={facet.name === 'source_type' &&
                                                selectedSourceTypes.has(facetValue.value)}
                                            onchange={() =>
                                                toggleFilter(facet.name, facetValue.value)}
                                            class="h-3 w-3 rounded border-gray-300 text-blue-600" />
                                        <span class="truncate text-gray-700">
                                            {getDisplayValue(facet.name, facetValue.value)}
                                        </span>
                                    </div>
                                    <span
                                        class="ml-2 rounded bg-gray-100 px-1 py-0.5 text-xs text-gray-500">
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
            }} />
    {/if}

    <div class="flex gap-6 px-6">
        <!-- Search Results -->
        <div class="min-w-0 flex-1">
            {#if data.searchResults}
                {#if data.searchResults.results.length > 0}
                    <div class="space-y-8">
                        {#each data.searchResults.results as result}
                            <SearchResultItem {result} {sourcesLookup} />
                        {/each}
                    </div>

                    <!-- Pagination -->
                    {#if totalPages > 1}
                        <div class="mt-8">
                            <Pagination.Root
                                count={data.searchResults.total_count}
                                perPage={data.pageSize}
                                page={data.currentPage}
                                onPageChange={(newPage) => navigateToPage(newPage)}
                                siblingCount={1}>
                                {#snippet children({ pages, currentPage })}
                                    <Pagination.Content>
                                        <Pagination.Item>
                                            <Pagination.Previous />
                                        </Pagination.Item>
                                        {#each pages as p (p.key)}
                                            {#if p.type === 'ellipsis'}
                                                <Pagination.Item>
                                                    <Pagination.Ellipsis />
                                                </Pagination.Item>
                                            {:else}
                                                <Pagination.Item>
                                                    <Pagination.Link
                                                        page={p}
                                                        isActive={currentPage === p.value}
                                                        class="cursor-pointer" />
                                                </Pagination.Item>
                                            {/if}
                                        {/each}
                                        <Pagination.Item>
                                            <Pagination.Next />
                                        </Pagination.Item>
                                    </Pagination.Content>
                                {/snippet}
                            </Pagination.Root>
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
                            <Button
                                variant="outline"
                                onclick={clearFilters}
                                class="mr-2 cursor-pointer">
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
                        class="mx-auto mb-4 h-8 w-8 animate-spin rounded-full border-4 border-gray-300 border-t-blue-600">
                    </div>
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
                        {#if activeSourceTypes.size > 0}
                            <Button
                                variant="ghost"
                                size="sm"
                                onclick={() => clearFacetFilters('source_type')}
                                class="cursor-pointer text-xs">
                                Clear
                            </Button>
                        {/if}
                    </div>
                    <div class="flex flex-col space-y-2">
                        {#each sourceFacet.values as facetValue}
                            {@const sourceIcon = getSourceIconPath(facetValue.value)}
                            {@const isActive = activeSourceTypes.has(facetValue.value)}
                            {@const isDisabled = activeSourceTypes.size > 0 && !isActive}
                            <Button
                                variant="ghost"
                                disabled={isDisabled}
                                class="flex justify-between rounded-full {isActive
                                    ? 'cursor-pointer bg-blue-50 hover:bg-blue-100'
                                    : isDisabled
                                      ? 'cursor-default opacity-50'
                                      : 'cursor-pointer hover:bg-gray-200'}"
                                onclick={() =>
                                    !isDisabled && toggleFilter('source_type', facetValue.value)}>
                                <div class="flex items-center gap-2">
                                    {#if sourceIcon}
                                        <img
                                            src={sourceIcon}
                                            alt="{facetValue.value} icon"
                                            class="h-4 w-4 {isDisabled ? 'grayscale' : ''}" />
                                    {:else}
                                        <FileText
                                            class="h-4 w-4 {isDisabled
                                                ? 'text-gray-300'
                                                : 'text-gray-400'}" />
                                    {/if}
                                    <span
                                        class="text-sm font-medium {isActive
                                            ? 'text-blue-700'
                                            : isDisabled
                                              ? 'text-gray-400'
                                              : 'text-gray-700'}">
                                        {getDisplayValue('source_type', facetValue.value)}
                                    </span>
                                </div>
                                <span
                                    class="rounded-full px-2 py-0.5 text-xs {isActive
                                        ? 'bg-blue-100 text-blue-700'
                                        : 'bg-gray-100 text-gray-500'}">
                                    {facetValue.count}
                                </span>
                            </Button>
                        {/each}
                    </div>
                </div>

                {#if getTotalSelectedFilters() > 0}
                    <div class="mt-4">
                        <Button
                            variant="outline"
                            size="sm"
                            onclick={clearFilters}
                            class="w-full cursor-pointer">
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
