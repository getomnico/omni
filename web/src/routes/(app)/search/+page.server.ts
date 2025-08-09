import { env } from '$env/dynamic/private'
import type { SearchResponse, SearchRequest } from '$lib/types/search.js'

export const load = async ({ url, fetch }) => {
    const query = url.searchParams.get('q')

    if (!query || query.trim() === '') {
        return {
            searchResults: null,
            sources: null,
        }
    }

    try {
        // Fetch search results and sources in parallel
        const [searchResponse, sourcesResponse] = await Promise.all([
            // Search request
            fetch(`${env.SEARCHER_URL}/search`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    query: query.trim(),
                    limit: 20,
                    offset: 0,
                    mode: 'hybrid',
                } as SearchRequest),
            }),
            // Sources request
            fetch('/api/sources', {
                method: 'GET',
                headers: {
                    'Content-Type': 'application/json',
                },
            }),
        ])

        if (!searchResponse.ok) {
            console.error(
                'Search request failed:',
                searchResponse.status,
                searchResponse.statusText,
            )
            return {
                searchResults: null,
                sources: null,
                error: 'Search service unavailable',
            }
        }

        const searchResults: SearchResponse = await searchResponse.json()
        let sources = null

        if (sourcesResponse.ok) {
            sources = await sourcesResponse.json()
        } else {
            console.warn(
                'Sources request failed:',
                sourcesResponse.status,
                sourcesResponse.statusText,
            )
        }

        console.log('Search Results:', JSON.stringify(searchResults, null, 2))
        return {
            searchResults,
            sources,
        }
    } catch (error) {
        console.error('Error performing search:', error)
        return {
            searchResults: null,
            sources: null,
            error: 'Failed to perform search',
        }
    }
}
