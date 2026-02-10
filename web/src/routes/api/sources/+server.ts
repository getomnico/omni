import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, serviceCredentials, syncRuns } from '$lib/server/db/schema'
import { eq, inArray, desc, sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { logger } from '$lib/server/logger'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const allSources = await db.query.sources.findMany()
    logger.debug(`/api/sources: found ${allSources.length} sources.`)

    // Get service credentials for all sources
    const sourceIds = allSources.map((s) => s.id)
    const credentials =
        sourceIds.length > 0
            ? await db.query.serviceCredentials.findMany({
                  where: inArray(serviceCredentials.sourceId, sourceIds),
              })
            : []

    // Get latest sync run for each source
    const latestSyncRuns =
        sourceIds.length > 0
            ? await db
                  .select()
                  .from(syncRuns)
                  .where(
                      sql`${syncRuns.id} IN (
                          SELECT DISTINCT ON (source_id) id
                          FROM sync_runs
                          WHERE source_id IN ${sourceIds}
                          ORDER BY source_id, started_at DESC
                      )`,
                  )
            : []
    logger.debug(`/api/sources: found ${latestSyncRuns.length} latest sync runs.`)

    const syncRunMap = new Map(latestSyncRuns.map((r) => [r.sourceId, r]))

    // Create a map of source ID to whether it has credentials
    const credentialsMap = new Map(credentials.map((c) => [c.sourceId, true]))

    const sanitizedSources = allSources.map((source) => {
        const latestSync = syncRunMap.get(source.id)
        return {
            id: source.id,
            name: source.name,
            sourceType: source.sourceType,
            config: source.config,
            syncStatus: latestSync?.status ?? null,
            isActive: source.isActive,
            lastSyncAt: latestSync?.completedAt ?? null,
            syncError: latestSync?.errorMessage ?? null,
            createdAt: source.createdAt,
            updatedAt: source.updatedAt,
            isConnected: credentialsMap.has(source.id),
        }
    })

    return json(sanitizedSources)
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const body = await request.json()
    const { name, sourceType, config, isActive } = body

    if (!name || !sourceType) {
        throw error(400, 'Name and sourceType are required')
    }

    const [newSource] = await db
        .insert(sources)
        .values({
            id: ulid(),
            name,
            sourceType,
            config: config || {},
            createdBy: locals.user.id,
            isActive: isActive ?? false,
        })
        .returning()

    return json({
        id: newSource.id,
        name: newSource.name,
        sourceType: newSource.sourceType,
        config: newSource.config,
        syncStatus: null,
        isActive: newSource.isActive,
        lastSyncAt: null,
        syncError: null,
        createdAt: newSource.createdAt,
        updatedAt: newSource.updatedAt,
        isConnected: false,
    })
}
