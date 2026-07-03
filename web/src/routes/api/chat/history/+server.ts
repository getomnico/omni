import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats'

const DEFAULT_LIMIT = 20
const MAX_LIMIT = 50

function parseNonNegativeInteger(value: string | null, fallback: number): number {
    if (value === null) return fallback
    const parsed = Number.parseInt(value, 10)
    return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback
}

function parseOptionalBoolean(value: string | null): boolean | undefined {
    if (value === null) return undefined
    if (value === 'true') return true
    if (value === 'false') return false
    return undefined
}

export const GET: RequestHandler = async ({ url, locals }) => {
    const logger = locals.logger.child('chat-history')

    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const requestedLimit = parseNonNegativeInteger(url.searchParams.get('limit'), DEFAULT_LIMIT)
    const limit = Math.min(Math.max(requestedLimit, 1), MAX_LIMIT)
    const offset = parseNonNegativeInteger(url.searchParams.get('offset'), 0)
    const isStarred = parseOptionalBoolean(url.searchParams.get('isStarred'))

    try {
        const rows = await chatRepository.getByUserId(locals.user.id, {
            limit: limit + 1,
            offset,
            isStarred,
        })
        const hasMore = rows.length > limit
        const items = hasMore ? rows.slice(0, limit) : rows

        return json({
            items,
            nextOffset: hasMore ? offset + items.length : null,
            hasMore,
        })
    } catch (error) {
        logger.error('Error loading chat history', error)
        return json(
            {
                error: 'History load failed',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
