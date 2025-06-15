import { SEARCHER_URL } from '$env/static/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ url, fetch }) => {
    const query = url.searchParams.get('q')
    const limitParam = url.searchParams.get('limit')

    if (!query || query.trim() === '') {
        return json({ error: 'Query parameter (q) is required' }, { status: 400 })
    }

    const limit = limitParam ? Math.min(parseInt(limitParam), 20) : 5

    try {
        const searchUrl = new URL(`${SEARCHER_URL}/suggestions`)
        searchUrl.searchParams.set('q', query.trim())
        searchUrl.searchParams.set('limit', limit.toString())

        const response = await fetch(searchUrl.toString())

        if (!response.ok) {
            console.error('Suggestions service error:', response.status, response.statusText)
            return json({ 
                error: 'Suggestions service unavailable',
                details: `Status: ${response.status}`
            }, { status: 502 })
        }

        const suggestions = await response.json()
        return json(suggestions)

    } catch (error) {
        console.error('Error calling suggestions service:', error)
        return json({ 
            error: 'Failed to get suggestions',
            details: error instanceof Error ? error.message : 'Unknown error'
        }, { status: 500 })
    }
}