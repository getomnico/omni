import { env } from '$env/dynamic/private'
import type { SearchResponse, SearchRequest } from '$lib/types/search.js'

export const load = async ({ url, fetch, locals }) => {
    const query = url.searchParams.get('q')
    const aiAnswerEnabled = env.AI_ANSWER_ENABLED !== 'false' // Default to true if not set

    // Parse source_type filter from URL params (can be multiple)
    const sourceTypes = url.searchParams.getAll('source_type')

    if (!query || query.trim() === '') {
        return {
            searchResults: null,
            sources: null,
            aiAnswerEnabled,
            selectedSourceTypes: sourceTypes,
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
                    user_id: locals.user?.id,
                    user_email: locals.user?.email,
                    source_types: sourceTypes.length > 0 ? sourceTypes : undefined,
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
                aiAnswerEnabled,
                selectedSourceTypes: sourceTypes,
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

        return {
            searchResults,
            sources,
            aiAnswerEnabled,
            selectedSourceTypes: sourceTypes,
        }
    } catch (error) {
        console.error('Error performing search:', error)
        return {
            searchResults: null,
            sources: null,
            error: 'Failed to perform search',
            aiAnswerEnabled,
            selectedSourceTypes: sourceTypes,
        }
    }
}
