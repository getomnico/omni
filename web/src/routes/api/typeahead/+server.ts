import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ fetch, locals, url }) => {
    if (!locals.user?.id) {
        return json({ results: [], query: '' })
    }

    const query = url.searchParams.get('q') || ''
    const limit = url.searchParams.get('limit') || '5'

    try {
        const typeaheadUrl = new URL(`${env.SEARCHER_URL}/typeahead`)
        typeaheadUrl.searchParams.set('q', query)
        typeaheadUrl.searchParams.set('limit', limit)
        typeaheadUrl.searchParams.set('user_id', locals.user.id)

        const response = await fetch(typeaheadUrl.toString())

        if (!response.ok) {
            console.error('Typeahead service error:', response.status, response.statusText)
            return json({ results: [], query })
        }

        const data = await response.json()
        return json(data)
    } catch (error) {
        console.error('Error calling typeahead service:', error)
        return json({ results: [], query })
    }
}
