import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'

export const POST: RequestHandler = async ({ request, fetch, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const logger = locals.logger.child('v1-search')

    let body: Record<string, unknown>
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const query = typeof body.query === 'string' ? body.query.trim() : ''
    if (!query) {
        return json({ error: 'query is required' }, { status: 400 })
    }

    // Enforce API key source scoping
    const allowedSources = locals.apiKeyAllowedSources
    let sourceTypes: string[] | undefined = Array.isArray(body.source_types)
        ? body.source_types
        : undefined

    if (allowedSources) {
        if (sourceTypes) {
            // Intersect: only allow sources that are both requested AND permitted
            sourceTypes = sourceTypes.filter((s: string) => allowedSources.includes(s))
            if (sourceTypes.length === 0) {
                return json({ results: [], total_count: 0, query_time_ms: 0, has_more: false, query })
            }
        } else {
            // No explicit filter — restrict to allowed sources
            sourceTypes = allowedSources
        }
    }

    const queryData = {
        query,
        source_types: sourceTypes,
        content_types: Array.isArray(body.content_types) ? body.content_types : undefined,
        limit: typeof body.limit === 'number' ? Math.min(body.limit, 100) : 20,
        offset: typeof body.offset === 'number' ? body.offset : 0,
        mode: ['fulltext', 'semantic', 'hybrid'].includes(body.mode as string)
            ? body.mode
            : 'hybrid',
        // 'admin' scope: omit user_email → searcher skips permission filter → all docs
        // 'user' scope (or cookie auth): real user identity → user's permitted docs
        // 'public' scope: sentinel email → only public docs
        user_email:
            locals.apiKeyScope === 'admin'
                ? undefined
                : locals.apiKeyScope === 'public'
                  ? '__public_access__@omni.internal'
                  : locals.user.email,
        user_id:
            locals.apiKeyScope === 'admin' || locals.apiKeyScope === 'public'
                ? undefined
                : locals.user.id,
    }

    logger.debug('Agent search request', { query: queryData.query, mode: queryData.mode })

    try {
        const response = await fetch(`${env.SEARCHER_URL}/search`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(queryData),
        })

        if (!response.ok) {
            logger.error('Searcher service error', undefined, {
                status: response.status,
                query: queryData.query,
            })
            return json(
                { error: 'Search service unavailable', status: response.status },
                { status: 502 },
            )
        }

        const results = await response.json()

        logger.info('Agent search completed', {
            query: queryData.query,
            results_count: results.results?.length ?? 0,
            total_count: results.total_count,
            query_time_ms: results.query_time_ms,
        })

        return json(results)
    } catch (error) {
        logger.error('Search request failed', error as Error, { query: queryData.query })
        return json({ error: 'Search request failed' }, { status: 500 })
    }
}
