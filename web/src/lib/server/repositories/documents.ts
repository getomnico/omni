import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import * as schema from '$lib/server/db/schema'
import { documents } from '$lib/server/db/schema'
import { sql } from 'drizzle-orm'

export class DocumentsRepository {
    async getCountsBySource(db: PostgresJsDatabase<typeof schema>) {
        return await db
            .select({
                sourceId: documents.sourceId,
                count: sql<number>`COUNT(*)::int`,
            })
            .from(documents)
            .groupBy(documents.sourceId)
    }
}

export const documentsRepository = new DocumentsRepository()
