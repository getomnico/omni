import { db } from '$lib/server/db'
import { syncRuns, sources } from '$lib/server/db/schema'
import { eq, desc } from 'drizzle-orm'

export class SyncRunsRepository {
    async getLatest(limit: number = 10) {
        return await db
            .select({
                id: syncRuns.id,
                sourceId: syncRuns.sourceId,
                sourceName: sources.name,
                sourceType: sources.sourceType,
                syncType: syncRuns.syncType,
                status: syncRuns.status,
                documentsProcessed: syncRuns.documentsProcessed,
                documentsUpdated: syncRuns.documentsUpdated,
                startedAt: syncRuns.startedAt,
                completedAt: syncRuns.completedAt,
                errorMessage: syncRuns.errorMessage,
            })
            .from(syncRuns)
            .leftJoin(sources, eq(syncRuns.sourceId, sources.id))
            .orderBy(desc(syncRuns.startedAt))
            .limit(limit)
    }

    async getLatestForSourceId(sourceId: string, limit: number = 10) {
        return await db
            .select()
            .from(syncRuns)
            .where(eq(syncRuns.sourceId, sourceId))
            .orderBy(desc(syncRuns.startedAt), desc(syncRuns.createdAt))
            .limit(limit)
    }
}

export const syncRunsRepository = new SyncRunsRepository()
