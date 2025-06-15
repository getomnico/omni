<script lang="ts">
    import { page } from '$app/stores'
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import { Search, FileText, Calendar, User } from '@lucide/svelte'
    import type { PageData } from './$types.js'
    import type { SearchResponse } from '$lib/types/search.js'

    export let data: PageData

    let searchQuery = $page.url.searchParams.get('q') || ''
    let isLoading = false

    function handleSearch() {
        if (searchQuery.trim()) {
            window.location.href = `/search?q=${encodeURIComponent(searchQuery.trim())}`
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
                    bind:value={searchQuery}
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

        {#if data.searchResults}
            <div class="text-sm text-gray-600">
                Found {data.searchResults.total_count} results in {data.searchResults.query_time_ms}ms for "{data.searchResults.query}"
            </div>
        {/if}
    </div>

    <!-- Search Results -->
    {#if data.searchResults}
        {#if data.searchResults.results.length > 0}
            <div class="space-y-4">
                {#each data.searchResults.results as result}
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
            {#if data.searchResults.has_more}
                <div class="mt-8 text-center">
                    <Button variant="outline">Load More Results</Button>
                </div>
            {/if}
        {:else}
            <div class="text-center py-12">
                <Search class="h-12 w-12 text-gray-400 mx-auto mb-4" />
                <h3 class="text-lg font-medium text-gray-900 mb-2">No results found</h3>
                <p class="text-gray-600 mb-4">
                    Try adjusting your search terms or check if your data sources are connected and indexed.
                </p>
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