import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, syncRuns, serviceCredentials } from '$lib/server/db/schema'
import { eq, and } from 'drizzle-orm'
import { getConfig } from '$lib/server/config'
import { logger } from '$lib/server/logger'

export const DELETE: RequestHandler = async ({ params, locals, fetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = params.sourceId

    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (locals.user.role !== 'admin' && source.createdBy !== locals.user.id) {
        throw error(403, 'Admin access required')
    }

    const config = getConfig()
    const connectorManagerUrl = config.services.connectorManagerUrl

    // Cancel any running sync for this source
    const runningSyncs = await db.query.syncRuns.findMany({
        where: and(eq(syncRuns.sourceId, sourceId), eq(syncRuns.status, 'running')),
    })

    for (const sync of runningSyncs) {
        try {
            await fetch(`${connectorManagerUrl}/sync/${sync.id}/cancel`, {
                method: 'POST',
            })
        } catch (err) {
            logger.warn(`Failed to cancel sync ${sync.id} for source ${sourceId}`, err)
        }
    }

    // Delete service credentials eagerly (small table, contains sensitive OAuth tokens)
    await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

    // Soft-delete the source — background cleanup in connector-manager will handle documents/embeddings
    await db
        .update(sources)
        .set({
            isActive: false,
            isDeleted: true,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))

    return json({ success: true })
}
