import { json } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import { parseDocumentPreview } from '$lib/server/document-preview'
import type { RequestHandler } from './$types'

export const GET: RequestHandler = async ({ params, fetch, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    const logger = locals.logger.child('document-preview')
    const documentId = params.id

    try {
        // Always scoped to the caller's own identity: previews never use admin access.
        // An empty query makes the searcher return the stored content instead of
        // running a search inside the document.
        const response = await fetch(`${env.SEARCHER_URL}/search`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                query: '',
                document_id: documentId,
                user_id: locals.user.id,
                user_email: locals.user.email,
                limit: 1,
            }),
        })

        if (!response.ok) {
            // The searcher reports missing and inaccessible documents alike as a 500.
            if (response.status === 500) {
                return json({ error: 'Document not found' }, { status: 404 })
            }
            logger.error('Searcher service error', undefined, {
                status: response.status,
                documentId,
            })
            return json({ error: 'Service unavailable' }, { status: 502 })
        }

        const preview = parseDocumentPreview(await response.json())
        if (!preview) {
            return json({ error: 'Document content not available' }, { status: 404 })
        }

        const allowedSources = locals.apiKeyAllowedSources
        if (
            allowedSources &&
            (!preview.sourceType || !allowedSources.includes(preview.sourceType))
        ) {
            return json({ error: 'Access denied for this source type' }, { status: 403 })
        }

        return json(preview)
    } catch (error) {
        logger.error('Document preview request failed', error as Error, { documentId })
        return json({ error: 'Failed to fetch document' }, { status: 500 })
    }
}
