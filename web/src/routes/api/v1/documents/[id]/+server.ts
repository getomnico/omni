import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ params, url, fetch, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const logger = locals.logger.child('v1-documents')
    const documentId = params.id

    if (!documentId) {
        return json({ error: 'Document ID is required' }, { status: 400 })
    }

    // Optional line range for large documents
    const startLine = url.searchParams.get('start_line')
    const endLine = url.searchParams.get('end_line')

    const queryData: Record<string, unknown> = {
        query: 'content',
        document_id: documentId,
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
        limit: 1,
    }

    if (startLine) {
        const parsed = parseInt(startLine, 10)
        if (!isNaN(parsed) && parsed >= 0) {
            queryData.document_content_start_line = parsed
        }
    }
    if (endLine) {
        const parsed = parseInt(endLine, 10)
        if (!isNaN(parsed) && parsed > 0) {
            queryData.document_content_end_line = parsed
        }
    }

    logger.debug('Document content request', { documentId })

    try {
        const response = await fetch(`${env.SEARCHER_URL}/search`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(queryData),
        })

        if (!response.ok) {
            // Searcher returns 500 for nonexistent document IDs (no dedicated 404)
            if (response.status === 500) {
                return json({ error: 'Document not found' }, { status: 404 })
            }
            logger.error('Searcher service error', undefined, {
                status: response.status,
                documentId,
            })
            return json({ error: 'Service unavailable' }, { status: 502 })
        }

        const searchResults = await response.json()
        const results = searchResults.results ?? []

        if (results.length === 0) {
            return json({ error: 'Document not found' }, { status: 404 })
        }

        const result = results[0]
        const doc = result.document

        // The searcher returns content in different fields depending on the retrieval mode:
        // - full_content: content is in highlights[0]
        // - line_range: line-numbered content is in highlights[0]
        // - chunk-based: content spread across multiple results' highlights
        let content: string | null = null
        if (results.length === 1 && result.highlights?.length > 0) {
            content = result.highlights.join('\n')
        } else if (results.length > 1) {
            // Multiple chunks — join them all
            content = results
                .map((r: { highlights?: string[] }) => (r.highlights ?? []).join('\n'))
                .join('\n\n')
        }

        // Enforce API key source scoping
        const docSourceType = doc.attributes?.source_type ?? result.source_type
        const allowedSources = locals.apiKeyAllowedSources
        if (allowedSources && docSourceType && !allowedSources.includes(docSourceType)) {
            return json({ error: 'Access denied for this source type' }, { status: 403 })
        }

        return json({
            id: doc.id,
            title: doc.title,
            url: doc.url,
            source_type: docSourceType,
            content_type: doc.content_type,
            content,
            match_type: result.match_type,
            metadata: doc.metadata ?? {},
            created_at: doc.created_at,
            updated_at: doc.updated_at,
        })
    } catch (error) {
        logger.error('Document content request failed', error as Error, { documentId })
        return json({ error: 'Failed to fetch document' }, { status: 500 })
    }
}
