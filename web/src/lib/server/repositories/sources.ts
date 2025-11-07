import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import type { Source } from '$lib/server/db/schema'

export class SourcesRepository {
    async getAll(): Promise<Source[]> {
        return await db.select().from(sources).where(eq(sources.isActive, true))
    }

    async getById(sourceId: string): Promise<Source | null> {
        const result = await db.select().from(sources).where(eq(sources.id, sourceId)).limit(1)
        return result[0] ?? null
    }
}

export const sourcesRepository = new SourcesRepository()
