import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { GoogleConnectorOAuthService } from '$lib/server/oauth/googleConnector'

/// Initiates the per-user OAuth flow that attaches the acting user's write
/// credentials to an existing org-wide source. Redirects to the provider's
/// consent screen with write scopes; the callback writes a per-user
/// service_credentials row scoped to (source_id, user_id).
export const GET: RequestHandler = async ({ params, locals, url }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = params.sourceId
    if (!sourceId) {
        throw error(400, 'sourceId is required')
    }

    const [source] = await db
        .select({
            id: sources.id,
            sourceType: sources.sourceType,
            scope: sources.scope,
            isDeleted: sources.isDeleted,
        })
        .from(sources)
        .where(and(eq(sources.id, sourceId), eq(sources.isDeleted, false)))
        .limit(1)

    if (!source) {
        throw error(404, 'Source not found')
    }
    if (source.scope !== 'org') {
        throw error(
            400,
            'Per-user authentication is only supported for org-wide sources. Personal sources already use the owner credential.',
        )
    }

    const returnTo = url.searchParams.get('return_to') ?? undefined

    // Provider-specific dispatch. v1 supports Google source types; extend as we
    // wire up additional connectors for write tools.
    const sourceType = source.sourceType
    if (sourceType === 'google_drive' || sourceType === 'gmail') {
        const isConfigured = await GoogleConnectorOAuthService.isConfigured()
        if (!isConfigured) {
            throw error(
                412,
                'Google OAuth client is not configured. Ask an admin to set it up under Admin → Settings → Integrations.',
            )
        }

        const { url: authUrl } = await GoogleConnectorOAuthService.generateUserWriteAuthUrl(
            sourceId,
            sourceType,
            locals.user.id,
            returnTo,
        )

        throw redirect(302, authUrl)
    }

    throw error(501, `Per-user OAuth is not implemented for source_type=${sourceType} yet.`)
}
