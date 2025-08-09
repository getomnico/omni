import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, serviceCredentials } from '$lib/server/db/schema'
import { eq, inArray } from 'drizzle-orm'
import { ulid } from 'ulid'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const allSources = await db.query.sources.findMany()
    console.log(`/api/sources: found ${allSources.length} sources.`)

    // Get service credentials for all sources
    const sourceIds = allSources.map((s) => s.id)
    const credentials =
        sourceIds.length > 0
            ? await db.query.serviceCredentials.findMany({
                  where: inArray(serviceCredentials.sourceId, sourceIds),
              })
            : []

    // Create a map of source ID to whether it has credentials
    const credentialsMap = new Map(credentials.map((c) => [c.sourceId, true]))

    const sanitizedSources = allSources.map((source) => ({
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
        isConnected: credentialsMap.has(source.id),
    }))

    console.log(`Sources: `, sanitizedSources)

    return json(sanitizedSources)
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const body = await request.json()
    const { name, sourceType, config } = body

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
            isActive: true,
            syncStatus: 'pending',
        })
        .returning()

    return json({
        id: newSource.id,
        name: newSource.name,
        sourceType: newSource.sourceType,
        config: newSource.config,
        syncStatus: newSource.syncStatus,
        isActive: newSource.isActive,
        lastSyncAt: newSource.lastSyncAt,
        syncError: newSource.syncError,
        createdAt: newSource.createdAt,
        updatedAt: newSource.updatedAt,
        isConnected: false,
    })
}
