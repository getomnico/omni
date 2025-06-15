import { SEARCHER_URL } from '$env/static/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import type { SearchRequest } from '$lib/types/search.js'

export const POST: RequestHandler = async ({ request, fetch }) => {
    let searchRequest: SearchRequest

    try {
        searchRequest = await request.json()
    } catch (error) {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    if (!searchRequest.query || searchRequest.query.trim() === '') {
        return json({ error: 'Query parameter is required' }, { status: 400 })
    }

    try {
        const response = await fetch(`${SEARCHER_URL}/search`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                query: searchRequest.query.trim(),
                sources: searchRequest.sources,
                content_types: searchRequest.content_types,
                limit: searchRequest.limit || 20,
                offset: searchRequest.offset || 0,
                mode: searchRequest.mode || 'fulltext'
            })
        })

        if (!response.ok) {
            console.error('Search service error:', response.status, response.statusText)
            return json({ 
                error: 'Search service unavailable',
                details: `Status: ${response.status}`
            }, { status: 502 })
        }

        const searchResults = await response.json()
        return json(searchResults)

    } catch (error) {
        console.error('Error calling search service:', error)
        return json({ 
            error: 'Failed to perform search',
            details: error instanceof Error ? error.message : 'Unknown error'
        }, { status: 500 })
    }
}