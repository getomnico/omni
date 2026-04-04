import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, serviceCredentials, syncRuns, documents } from '$lib/server/db/schema'
import { eq, inArray, sql, count } from 'drizzle-orm'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    let allSources = await db.query.sources.findMany()

    // Filter by API key source scoping
    const allowedSources = locals.apiKeyAllowedSources
    if (allowedSources) {
        allSources = allSources.filter((s) => allowedSources.includes(s.sourceType))
    }

    const sourceIds = allSources.map((s) => s.id)

    // Run parallel queries for credentials, sync runs, and document counts
    const [credentials, latestSyncRuns, docCounts] = await Promise.all([
        sourceIds.length > 0
            ? db.query.serviceCredentials.findMany({
                  where: inArray(serviceCredentials.sourceId, sourceIds),
              })
            : Promise.resolve([]),
        sourceIds.length > 0
            ? db
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
            : Promise.resolve([]),
        sourceIds.length > 0
            ? db
                  .select({
                      sourceId: documents.sourceId,
                      count: count(),
                  })
                  .from(documents)
                  .where(inArray(documents.sourceId, sourceIds))
                  .groupBy(documents.sourceId)
            : Promise.resolve([]),
    ])

    const syncRunMap = new Map(latestSyncRuns.map((r) => [r.sourceId, r]))
    const credentialsMap = new Map(credentials.map((c) => [c.sourceId, true]))
    const docCountMap = new Map(docCounts.map((d) => [d.sourceId, d.count]))

    const result = allSources.map((source) => {
        const latestSync = syncRunMap.get(source.id)
        return {
            id: source.id,
            name: source.name,
            source_type: source.sourceType,
            is_active: source.isActive,
            is_connected: credentialsMap.has(source.id),
            document_count: docCountMap.get(source.id) ?? 0,
            sync_status: latestSync?.status ?? null,
            last_sync_at: latestSync?.completedAt ?? null,
            documents_scanned: latestSync?.documentsScanned ?? null,
            documents_processed: latestSync?.documentsProcessed ?? null,
            sync_error: latestSync?.errorMessage ?? null,
            created_at: source.createdAt,
        }
    })

    return json({ sources: result })
}
