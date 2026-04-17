import { db } from '$lib/server/db'
import { sources, syncRuns, user } from '$lib/server/db/schema'
import { eq, desc, sql, and, inArray, set } from 'drizzle-orm'
import type { Source, SyncRun, PgDatabase, AnyPgColumn, PgTable } from 'drizzle-orm'

export class SourcesRepository {
    constructor(private db: PgDatabase) {}

    async getAll(): Promise<Source[]> {
        return await this.db
            .select()
            .from(sources)
            .where(eq(sources.isDeleted, false))
            .orderBy(desc(sources.createdAt))
    }

    async getById(sourceId: string): Promise<Source | null> {
        const result = await this.db.select().from(sources).where(eq(sources.id, sourceId)).limit(1)
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
        return await this.db
            .select()
            .from(sources)
            .where(and(eq(sources.createdBy, userId), eq(sources.isDeleted, false)))
            .orderBy(desc(sources.createdAt))
    }

    async getOrgWide(): Promise<Source[]> {
        const adminUserIds = this.db
            .select({ id: user.id })
            .from(user)
            .where(eq(user.role, 'admin'))

        return await this.db
            .select()
            .from(sources)
            .where(and(inArray(sources.createdBy, adminUserIds), eq(sources.isDeleted, false)))
            .orderBy(desc(sources.createdAt))
    }

    async getLatestSyncRuns(): Promise<Map<string, SyncRun>> {
        const rows = await this.db
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

    async updateById(sourceId: string, updates: Partial<Pick<Source, 'isActive' | 'isDeleted'>>) {
        await this.db.update(sources).set(updates).where(eq(sources.id, sourceId))
    }
}
