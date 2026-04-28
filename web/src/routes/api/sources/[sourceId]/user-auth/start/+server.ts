import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { getUserAuthAdapter } from '$lib/server/oauth/userAuthAdapters'

/// Initiates the per-user OAuth flow that attaches the acting user's write
/// credentials to an existing org-wide source. Provider dispatch lives in
/// `userAuthAdapters` — this route stays connector-agnostic.
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

    const adapter = getUserAuthAdapter(source.sourceType)
    if (!adapter) {
        throw error(
            501,
            `Per-user OAuth is not implemented for source_type=${source.sourceType} yet.`,
        )
    }

    if (!(await adapter.isConfigured())) {
        throw error(
            412,
            `OAuth client for ${source.sourceType} is not configured. Ask an admin to set it up under Admin → Settings → Integrations.`,
        )
    }

    const returnTo = url.searchParams.get('return_to') ?? undefined
    const { url: authUrl } = await adapter.generateAuthUrl({
        sourceId,
        sourceType: source.sourceType,
        userId: locals.user.id,
        returnTo,
    })

    throw redirect(302, authUrl)
}
