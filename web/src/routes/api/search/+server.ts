import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import type { SearchRequest } from '$lib/types/search.js'

export const POST: RequestHandler = async ({ request, fetch, locals }) => {
    const logger = locals.logger.child('search-api')
    let searchRequest: SearchRequest

    try {
        searchRequest = await request.json()
    } catch (error) {
        logger.warn('Invalid JSON in search request', error)
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    if (!searchRequest.query || searchRequest.query.trim() === '') {
        logger.warn('Empty query parameter in search request')
        return json({ error: 'Query parameter is required' }, { status: 400 })
    }

    const queryData = {
        query: searchRequest.query.trim(),
        sources: searchRequest.sources,
        content_types: searchRequest.content_types,
        limit: searchRequest.limit || 20,
        offset: searchRequest.offset || 0,
        mode: searchRequest.mode || 'fulltext',
        user_email: locals.user?.email,
        user_id: locals.user?.id,
        user_configuration: locals.user?.configuration,
    }

    logger.debug('Sending search request to searcher service', {
        query: queryData.query,
        mode: queryData.mode,
    })

    try {
        const response = await fetch(`${env.SEARCHER_URL}/search`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify(queryData),
        })

        if (!response.ok) {
            logger.error('Search service error', undefined, {
                status: response.status,
                statusText: response.statusText,
                query: queryData.query,
            })
            return json(
                {
                    error: 'Search service unavailable',
                    details: `Status: ${response.status}`,
                },
                { status: 502 },
            )
        }

        const searchResults = await response.json()
        logger.info('Search completed successfully', {
            query: queryData.query,
            resultsCount: searchResults.results?.length || 0,
        })

        return json(searchResults)
    } catch (error) {
        logger.error('Error calling search service', error, { query: queryData.query })
        return json(
            {
                error: 'Failed to perform search',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
