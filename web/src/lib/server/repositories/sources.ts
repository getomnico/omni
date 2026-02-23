import { db } from '$lib/server/db'
import { sources, syncRuns } from '$lib/server/db/schema'
import { eq, desc, sql, and } from 'drizzle-orm'
import type { Source, SyncRun } from '$lib/server/db/schema'

export class SourcesRepository {
    async getAll(): Promise<Source[]> {
        return await db
            .select()
            .from(sources)
            .where(eq(sources.isDeleted, false))
            .orderBy(desc(sources.createdAt))
    }

    async getById(sourceId: string): Promise<Source | null> {
        const result = await db.select().from(sources).where(eq(sources.id, sourceId)).limit(1)
        return result[0] ?? null
    }

    async getByUserId(userId: string): Promise<Source[]> {
        return await db
            .select()
            .from(sources)
            .where(and(eq(sources.createdBy, userId), eq(sources.isDeleted, false)))
            .orderBy(desc(sources.createdAt))
    }

    async getLatestSyncRuns(): Promise<Map<string, SyncRun>> {
        const rows = await db
            .select()
            .from(syncRuns)
            .where(
                sql`${syncRuns.id} IN (
                    SELECT DISTINCT ON (source_id) id
                    FROM sync_runs
                    ORDER BY source_id, started_at DESC
                )`,
            )

        return new Map(rows.map((sync) => [sync.sourceId, sync]))
    }
}

export const sourcesRepository = new SourcesRepository()
