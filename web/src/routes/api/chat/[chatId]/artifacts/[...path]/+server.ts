import { env } from '$env/dynamic/private'
import { error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'
import { artifactResponseHeaders } from '$lib/server/artifacts.js'

export const GET: RequestHandler = async ({ params, url, locals }) => {
    const { chatId, path } = params
    const logger = locals.logger.child('artifacts')

    if (!chatId || !path) {
        throw error(400, 'Missing chatId or path')
    }

    // Auth check: verify chat exists and belongs to user
    const chat = await chatRepository.get(chatId)
    if (!chat) {
        throw error(404, 'Chat not found')
    }

    if (chat.userId !== locals.user?.id) {
        throw error(403, 'Forbidden')
    }

    const version = url.searchParams.get('v')
    const versionSuffix = version ? `?v=${encodeURIComponent(version)}` : ''

    try {
        const response = await fetch(
            `${env.AI_SERVICE_URL}/chat/${chatId}/artifacts/${path}${versionSuffix}`,
        )

        if (!response.ok) {
            logger.warn('Artifact proxy failed', {
                chatId,
                path,
                status: response.status,
            })
            throw error(response.status, 'Artifact not found')
        }

        const contentType = response.headers.get('content-type')
        if (!contentType) {
            throw error(502, 'Artifact response did not include a content type')
        }
        const body = await response.arrayBuffer()
        // Version-pinned artifact URLs are immutable; unpinned ones track the
        // latest file content and must not be cached long.
        const cacheControl = version
            ? 'private, max-age=31536000, immutable'
            : 'private, max-age=3600'

        // Generated HTML gets a CSP sandbox as a second isolation boundary;
        // it applies even when the artifact URL is opened directly.
        return new Response(body, {
            headers: artifactResponseHeaders(contentType, cacheControl),
        })
    } catch (err) {
        if (
            typeof err === 'object' &&
            err !== null &&
            'status' in err &&
            typeof err.status === 'number'
        ) {
            throw err
        }
        logger.error('Artifact proxy error', { err, chatId, path })
        throw error(502, 'Failed to fetch artifact')
    }
}
