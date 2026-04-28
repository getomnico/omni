import { db } from '$lib/server/db'
import { documents } from '$lib/server/db/schema'
import { sql } from 'drizzle-orm'
import type { PgDatabase } from 'drizzle-orm'

export class DocumentsRepository {
    constructor(private db: PgDatabase) {}

    async getCountsBySource() {
        return await this.db
            .select({
                sourceId: documents.sourceId,
                count: sql<number>`COUNT(*)::int`,
            })
            .from(documents)
            .groupBy(documents.sourceId)
    }
}
