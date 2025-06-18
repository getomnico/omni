import { SEARCHER_URL } from '$env/static/private'
import type { SearchResponse, SearchRequest } from '$lib/types/search.js'

export const load = async ({ url, fetch }) => {
    const query = url.searchParams.get('q')

    if (!query || query.trim() === '') {
        return {
            searchResults: null,
        }
    }

    try {
        const searchRequest: SearchRequest = {
            query: query.trim(),
            limit: 20,
            offset: 0,
            mode: 'hybrid',
        }

        const response = await fetch(`${SEARCHER_URL}/search`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify(searchRequest),
        })

        if (!response.ok) {
            console.error('Search request failed:', response.status, response.statusText)
            return {
                searchResults: null,
                error: 'Search service unavailable',
            }
        }

        const searchResults: SearchResponse = await response.json()

        return {
            searchResults,
        }
    } catch (error) {
        console.error('Error performing search:', error)
        return {
            searchResults: null,
            error: 'Failed to perform search',
        }
    }
}
