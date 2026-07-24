import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
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

    async findActiveByTypeAndCreator(
        sourceType: string,
        createdBy: string,
    ): Promise<Source | null> {
        const result = await db
            .select()
            .from(sources)
            .where(
                and(
                    eq(sources.sourceType, sourceType),
                    eq(sources.createdBy, createdBy),
                    eq(sources.isDeleted, false),
                ),
            )
            .limit(1)
        return result[0] ?? null
    }

    async getByUserId(userId: string): Promise<Source[]> {
        return await db
            .select()
            .from(sources)
            .where(
                and(
                    eq(sources.createdBy, userId),
                    eq(sources.scope, 'user'),
                    eq(sources.isDeleted, false),
                ),
            )
            .orderBy(desc(sources.createdAt))
    }

    async getOrgWide(): Promise<Source[]> {
        return await db
            .select()
            .from(sources)
            .where(and(eq(sources.scope, 'org'), eq(sources.isDeleted, false)))
            .orderBy(desc(sources.createdAt))
    }

    async getLatestSyncRunsForSourceIds(sourceIds: string[]): Promise<Map<string, SyncRun>> {
        if (sourceIds.length === 0) {
            return new Map()
        }

        const rows = await db.execute<SyncRun>(sql`
            SELECT sr.id,
                   sr.source_id AS "sourceId",
                   sr.sync_type AS "syncType",
                   sr.started_at AS "startedAt",
                   sr.completed_at AS "completedAt",
                   sr.status,
                   sr.documents_scanned AS "documentsScanned",
                   sr.documents_processed AS "documentsProcessed",
                   sr.documents_updated AS "documentsUpdated",
                   sr.error_message AS "errorMessage",
                   sr.created_at AS "createdAt",
                   sr.updated_at AS "updatedAt"
            FROM sources s
            CROSS JOIN LATERAL (
                SELECT *
                FROM sync_runs
                WHERE source_id = s.id
                ORDER BY started_at DESC
                LIMIT 1
            ) sr
            WHERE s.id IN ${sourceIds}
              AND s.is_deleted = false
        `)

        return new Map(rows.map((sync) => [sync.sourceId, sync]))
    }
}

export const sourcesRepository = new SourcesRepository()
