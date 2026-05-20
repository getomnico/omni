import { db } from '$lib/server/db'
import { getConfig } from '$lib/server/config'
import { sources, syncRuns } from '$lib/server/db/schema'
import { eq, desc, sql, and } from 'drizzle-orm'
import type { Source, SyncRun } from '$lib/server/db/schema'
import type { SourceSyncOverview } from '$lib/types'

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

    async getLatestSyncRunsForSourceIds(sourceIds: string[]): Promise<Map<string, SyncRun>> {
        if (sourceIds.length === 0) {
            return new Map()
        }

        const rows = await db
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

        return new Map(rows.map((sync) => [sync.sourceId, sync]))
    }

    async getSourceSyncOverview(
        sourceId: string,
        logger?: { error: (message: string, error?: unknown) => void },
    ): Promise<SourceSyncOverview | null> {
        try {
            const response = await fetch(`${getConfig().services.connectorManagerUrl}/sources`)
            if (!response.ok) {
                logger?.error('Failed to fetch source sync overview', { status: response.status })
                return null
            }

            const overviews: SourceSyncOverview[] = await response.json()
            return overviews.find((overview) => overview.source.id === sourceId) ?? null
        } catch (error) {
            logger?.error('Failed to fetch source sync overview', error)
            return null
        }
    }
}

export const sourcesRepository = new SourcesRepository()
