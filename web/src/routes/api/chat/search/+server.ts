import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats'

export const GET: RequestHandler = async ({ url, locals }) => {
    const logger = locals.logger.child('chat-search')

    const query = url.searchParams.get('q')?.trim()
    if (!query) {
        return json({ error: 'Query parameter "q" is required' }, { status: 400 })
    }

    try {
        const results = await chatRepository.search(locals.user.id, query)
        return json(results)
    } catch (error) {
        logger.error('Error searching chats', error)
        return json(
            {
                error: 'Search failed',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
