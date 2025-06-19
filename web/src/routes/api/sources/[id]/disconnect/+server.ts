import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, serviceCredentials } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    // Only admins can disconnect sources
    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const sourceId = params.id

    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    // Delete service credentials for this source
    await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

    // Mark source as inactive
    await db
        .update(sources)
        .set({
            syncStatus: 'pending',
            isActive: false,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))

    return json({ success: true })
}
