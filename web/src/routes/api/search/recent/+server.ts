import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ fetch, locals }) => {
    // Check if user is authenticated
    if (!locals.user?.id) {
        return json({ searches: [] })
    }

    try {
        const searchUrl = new URL(`${env.SEARCHER_URL}/recent-searches`)
        searchUrl.searchParams.set('user_id', locals.user.id)

        const response = await fetch(searchUrl.toString(), {
            headers: env.OMNI_INTERNAL_SERVICE_TOKEN
                ? { 'x-omni-internal-token': env.OMNI_INTERNAL_SERVICE_TOKEN }
                : undefined,
        })

        if (!response.ok) {
            console.error('Recent searches service error:', response.status, response.statusText)
            // Return empty searches on error
            return json({ searches: [] })
        }

        const recentSearches = await response.json()
        return json(recentSearches)
    } catch (error) {
        console.error('Error calling recent searches service:', error)
        // Return empty searches on error
        return json({ searches: [] })
    }
}
