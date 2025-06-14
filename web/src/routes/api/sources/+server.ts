import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const userSources = await db.query.sources.findMany({
        where: eq(sources.createdBy, locals.user.id),
    })

    const sanitizedSources = userSources.map((source) => ({
        id: source.id,
        name: source.name,
        sourceType: source.sourceType,
        config: source.config,
        syncStatus: source.syncStatus,
        isActive: source.isActive,
        lastSyncAt: source.lastSyncAt,
        syncError: source.syncError,
        createdAt: source.createdAt,
        updatedAt: source.updatedAt,
        isConnected: !!source.oauthCredentials,
    }))

    return json(sanitizedSources)
}
