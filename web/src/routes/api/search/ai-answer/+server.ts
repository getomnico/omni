import { env } from '$env/dynamic/private'
import { error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import type { SearchRequest } from '$lib/types/search.js'

export const POST: RequestHandler = async ({ request, fetch, locals }) => {
    // Check if AI answers are enabled
    if (env.AI_ANSWER_ENABLED === 'false') {
        throw error(503, 'AI answers are currently disabled')
    }

    let searchRequest: SearchRequest

    try {
        searchRequest = await request.json()
    } catch (err) {
        throw error(400, 'Invalid JSON in request body')
    }

    if (!searchRequest.query || searchRequest.query.trim() === '') {
        throw error(400, 'Query parameter is required')
    }

    try {
        // Call the searcher AI answer endpoint
        const response = await fetch(`${env.SEARCHER_URL}/search/ai-answer`, {
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
                mode: 'hybrid', // Always use hybrid for AI answers
                user_email: locals.user?.email,
            }),
        })

        if (!response.ok) {
            console.error('AI answer service error:', response.status, response.statusText)
            throw error(502, `Search service unavailable: ${response.status}`)
        }

        // Return the streaming response directly
        return new Response(response.body, {
            status: 200,
            headers: {
                'Content-Type': 'text/plain; charset=utf-8',
                'Cache-Control': 'no-cache',
                Connection: 'keep-alive',
                // Add CORS headers if needed
                'Access-Control-Allow-Origin': '*',
                'Access-Control-Allow-Methods': 'POST',
                'Access-Control-Allow-Headers': 'Content-Type',
            },
        })
    } catch (err) {
        console.error('Error calling AI answer service:', err)
        if (err instanceof Error && 'status' in err) {
            throw err // Re-throw SvelteKit errors
        }
        throw error(500, 'Failed to generate AI answer')
    }
}
