import { db } from '$lib/server/db'
import { sources, syncRuns } from '$lib/server/db/schema'
import { eq, and } from 'drizzle-orm'
import type { Source, SyncRun } from '$lib/server/db/schema'

export class SourcesRepository {
    async getAll(): Promise<Source[]> {
        return await db.select().from(sources).where(eq(sources.isDeleted, false))
    }

    async getById(sourceId: string): Promise<Source | null> {
        const result = await db.select().from(sources).where(eq(sources.id, sourceId)).limit(1)
        return result[0] ?? null
    }

    async getRunningSyncs(): Promise<Map<string, SyncRun>> {
        const running = await db.select().from(syncRuns).where(eq(syncRuns.status, 'running'))

        return new Map(running.map((sync) => [sync.sourceId, sync]))
    }
}

export const sourcesRepository = new SourcesRepository()
