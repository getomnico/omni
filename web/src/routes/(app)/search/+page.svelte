<script lang="ts">
    import { page } from '$app/stores'
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Search, FileText, Calendar, User, Filter } from '@lucide/svelte'
    import type { PageData } from './$types.js'
    import type { SearchResponse, SearchRequest } from '$lib/types/search.js'
    import AIAnswer from '$lib/components/AIAnswer.svelte'

    let { data }: { data: PageData } = $props()

    // inputQuery represents the current value in the search input
    let inputQuery = $state($page.url.searchParams.get('q') || '')
    // searchQuery represents the submitted query (from URL param)
    let searchQuery = $state($page.url.searchParams.get('q') || '')
    let isLoading = $state(false)
    let selectedFilters = $state<Map<string, Set<string>>>(new Map())

    const facetDisplayNames: Record<string, string> = {
        'source_type': 'Source Type'
    }

    const sourceDisplayNames: Record<string, string> = {
        'google_drive': 'Google Drive',
        'gmail': 'Gmail',
        'confluence': 'Confluence',  
        'jira': 'JIRA',
        'slack': 'Slack',
        'github': 'GitHub',
        'local_files': 'Local Files'
    }

    let allFacets = $derived(data.searchResults?.facets || [])
    let sourceFacet = $derived(allFacets.find(f => f.name === 'source_type'))
    let otherFacets = $derived(allFacets.filter(f => f.name !== 'source_type'))
    let filteredResults = $derived(data.searchResults ? filterResults(data.searchResults, selectedFilters) : null)

    function getDisplayValue(facetField: string, value: string): string {
        if (facetField === 'source_type') {
            return sourceDisplayNames[value] || value
        }
        return value
    }

    function filterResults(searchResults: SearchResponse, selectedFilters: Map<string, Set<string>>): SearchResponse {
        if (selectedFilters.size === 0) {
            return searchResults
        }
        
        const filteredResults = searchResults.results.filter(result => {
            // Check if result matches all selected filters
            for (const [facetField, selectedValues] of selectedFilters) {
                if (selectedValues.size === 0) continue
                
                let fieldValue: string
                switch (facetField) {
                    case 'source_type':
                        // For now, we'll need to map source_id to source_type
                        // This is a simplified approach - in practice we'd need the actual source_type from the backend
                        fieldValue = result.document.source
                        break
                    default:
                        continue
                }
                
                if (!selectedValues.has(fieldValue)) {
                    return false
                }
            }
            return true
        })
        
        return {
            ...searchResults,
            results: filteredResults,
            total_count: filteredResults.length
        }
    }


    function toggleFilter(facetField: string, value: string) {
        const currentFilters = selectedFilters.get(facetField) || new Set()
        
        if (currentFilters.has(value)) {
            currentFilters.delete(value)
        } else {
            currentFilters.add(value)
        }
        
        if (currentFilters.size === 0) {
            selectedFilters.delete(facetField)
        } else {
            selectedFilters.set(facetField, currentFilters)
        }
        
        selectedFilters = new Map(selectedFilters)
    }

    function clearFilters() {
        selectedFilters = new Map()
    }

    function clearFacetFilters(facetField: string) {
        selectedFilters.delete(facetField)
        selectedFilters = new Map(selectedFilters)
    }

    function getTotalSelectedFilters(): number {
        let total = 0
        for (const filterSet of selectedFilters.values()) {
            total += filterSet.size
        }
        return total
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
</script>

<svelte:head>
    <title>Search Results - Clio</title>
</svelte:head>

<div class="container mx-auto px-4 py-6">
    <!-- Search Header -->
    <div class="mb-8">
        <div class="flex items-center gap-4 mb-4">
            <div class="flex items-center flex-1 rounded-lg border border-gray-300 bg-white shadow-sm">
                <div class="p-3">
                    <Search class="h-5 w-5 text-gray-400" />
                </div>
                <Input
                    type="text"
                    bind:value={inputQuery}
                    placeholder="Search across your organization..."
                    class="flex-1 border-none bg-transparent shadow-none focus:ring-0 focus-visible:ring-0"
                    onkeypress={handleKeyPress}
                />
                <Button 
                    class="m-2 px-6" 
                    onclick={handleSearch}
                    disabled={isLoading}
                >
                    {isLoading ? 'Searching...' : 'Search'}
                </Button>
            </div>
        </div>

        {#if filteredResults && data.searchResults}
            <div class="text-sm text-gray-600">
                Found {filteredResults.total_count} results in {data.searchResults.query_time_ms}ms for "{data.searchResults.query}"
                {#if getTotalSelectedFilters() > 0}
                    <span class="ml-2">• {getTotalSelectedFilters()} filter{getTotalSelectedFilters() > 1 ? 's' : ''} applied</span>
                {/if}
            </div>
        {/if}
    </div>

    <!-- Other Facets (above search results) -->
    {#if filteredResults && otherFacets.length > 0}
        <div class="mb-6">
            <div class="flex flex-wrap gap-4">
                {#each otherFacets as facet}
                    <div class="bg-white border rounded-lg p-4 min-w-48">
                        <div class="flex items-center justify-between mb-3">
                            <h3 class="text-sm font-medium text-gray-900">
                                {facetDisplayNames[facet.name] || facet.name}
                            </h3>
                            {#if selectedFilters.has(facet.name) && selectedFilters.get(facet.name)?.size > 0}
                                <Button 
                                    variant="ghost" 
                                    size="sm" 
                                    onclick={() => clearFacetFilters(facet.name)}
                                    class="text-xs h-6 px-2"
                                >
                                    Clear
                                </Button>
                            {/if}
                        </div>
                        <div class="space-y-2 max-h-32 overflow-y-auto">
                            {#each facet.values.slice(0, 5) as facetValue}
                                <label class="flex items-center justify-between cursor-pointer hover:bg-gray-50 p-1 rounded text-xs">
                                    <div class="flex items-center gap-2">
                                        <input 
                                            type="checkbox"
                                            checked={selectedFilters.get(facet.name)?.has(facetValue.value) || false}
                                            onchange={() => toggleFilter(facet.name, facetValue.value)}
                                            class="h-3 w-3 text-blue-600 rounded border-gray-300"
                                        />
                                        <span class="text-gray-700 truncate">
                                            {getDisplayValue(facet.name, facetValue.value)}
                                        </span>
                                    </div>
                                    <span class="text-gray-500 bg-gray-100 px-1 py-0.5 rounded text-xs ml-2">
                                        {facetValue.count}
                                    </span>
                                </label>
                            {/each}
                            {#if facet.values.length > 5}
                                <div class="text-xs text-gray-500 text-center pt-1">
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
    {#if filteredResults && searchQuery.trim()}
        <AIAnswer searchRequest={{
            query: searchQuery,
            limit: 20,
            offset: 0,
            mode: 'hybrid'
        }} />
    {/if}

    <div class="flex gap-6">
        <!-- Search Results -->
        <div class="flex-1">
            {#if filteredResults}
                {#if filteredResults.results.length > 0}
                    <div class="space-y-4">
                        {#each filteredResults.results as result}
                    <Card class="hover:shadow-md transition-shadow">
                        <CardContent class="p-6">
                            <div class="flex items-start justify-between mb-3">
                                <div class="flex items-center gap-2">
                                    <FileText class="h-4 w-4 text-blue-600" />
                                    <h3 class="text-lg font-semibold text-blue-600 hover:text-blue-800">
                                        <a href={result.document.url || '#'} target="_blank" rel="noopener noreferrer">
                                            {result.document.title}
                                        </a>
                                    </h3>
                                </div>
                                <div class="flex items-center gap-4 text-sm text-gray-500">
                                    <div class="flex items-center gap-1">
                                        <User class="h-3 w-3" />
                                        {result.document.source}
                                    </div>
                                    <div class="flex items-center gap-1">
                                        <Calendar class="h-3 w-3" />
                                        {formatDate(result.document.created_at)}
                                    </div>
                                    <div class="px-2 py-1 bg-gray-100 rounded text-xs">
                                        Score: {result.score.toFixed(2)}
                                    </div>
                                </div>
                            </div>

                            <div class="text-gray-700 mb-3">
                                {truncateContent(result.document.content)}
                            </div>

                            {#if result.highlights.length > 0}
                                <div class="mb-3">
                                    <h4 class="text-sm font-medium text-gray-900 mb-2">Highlights:</h4>
                                    <div class="space-y-1">
                                        {#each result.highlights as highlight}
                                            <div class="text-sm text-gray-600 bg-yellow-50 p-2 rounded">
                                                {@html highlight}
                                            </div>
                                        {/each}
                                    </div>
                                </div>
                            {/if}

                            <div class="text-xs text-gray-500">
                                Match type: {result.match_type} • Content type: {result.document.content_type}
                            </div>
                        </CardContent>
                            </Card>
                        {/each}
                    </div>

                    <!-- Pagination -->
                    {#if data.searchResults?.has_more}
                        <div class="mt-8 text-center">
                            <Button variant="outline">Load More Results</Button>
                        </div>
                    {/if}
                {:else}
                    <div class="text-center py-12">
                        <Search class="h-12 w-12 text-gray-400 mx-auto mb-4" />
                        <h3 class="text-lg font-medium text-gray-900 mb-2">No results found</h3>
                        <p class="text-gray-600 mb-4">
                            {#if getTotalSelectedFilters() > 0}
                                No results found with the current filters. Try clearing filters or adjusting your search.
                            {:else}
                                Try adjusting your search terms or check if your data sources are connected and indexed.
                            {/if}
                        </p>
                        {#if getTotalSelectedFilters() > 0}
                            <Button variant="outline" onclick={clearFilters} class="mr-2">
                                Clear Filters
                            </Button>
                        {/if}
                        <Button variant="outline" onclick={() => window.location.href = '/'}>
                            Back to Home
                        </Button>
                    </div>
                {/if}
            {:else if $page.url.searchParams.get('q')}
                <div class="text-center py-12">
                    <div class="animate-spin h-8 w-8 border-4 border-gray-300 border-t-blue-600 rounded-full mx-auto mb-4"></div>
                    <p class="text-gray-600">Searching...</p>
                </div>
            {:else}
                <div class="text-center py-12">
                    <Search class="h-12 w-12 text-gray-400 mx-auto mb-4" />
                    <h3 class="text-lg font-medium text-gray-900 mb-2">Enter a search query</h3>
                    <p class="text-gray-600">
                        Search across your organization's documents, emails, and more.
                    </p>
                </div>
            {/if}
        </div>

        <!-- Source Facets Sidebar -->
        {#if data.searchResults && sourceFacet}
            <div class="w-80">
                <Card>
                    <CardHeader>
                        <div class="flex items-center justify-between">
                            <CardTitle class="text-base flex items-center gap-2">
                                <Filter class="h-4 w-4" />
                                Filter by Source
                            </CardTitle>
                            {#if selectedFilters.has('source_type') && selectedFilters.get('source_type')?.size > 0}
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
                    </CardHeader>
                    <CardContent class="space-y-3">
                        {#each sourceFacet.values as facetValue}
                            <label class="flex items-center justify-between cursor-pointer hover:bg-gray-50 p-2 rounded">
                                <div class="flex items-center gap-2">
                                    <input 
                                        type="checkbox"
                                        checked={selectedFilters.get('source_type')?.has(facetValue.value) || false}
                                        onchange={() => toggleFilter('source_type', facetValue.value)}
                                        class="h-4 w-4 text-blue-600 rounded border-gray-300 focus:ring-blue-500"
                                    />
                                    <span class="text-sm font-medium text-gray-700">
                                        {getDisplayValue('source_type', facetValue.value)}
                                    </span>
                                </div>
                                <span class="text-xs text-gray-500 bg-gray-100 px-2 py-1 rounded">
                                    {facetValue.count}
                                </span>
                            </label>
                        {/each}
                    </CardContent>
                </Card>
                
                {#if getTotalSelectedFilters() > 0}
                    <div class="mt-4">
                        <Button 
                            variant="outline" 
                            size="sm" 
                            onclick={clearFilters}
                            class="w-full"
                        >
                            Clear All Filters
                        </Button>
                    </div>
                {/if}
            </div>
        {/if}
    </div>
</div>