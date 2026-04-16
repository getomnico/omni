import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

/**
 * Get a configuration value by key. Returns null if not found.
 */
export async function getConfigValue(key: string): Promise<Record<string, unknown> | null> {
    const [row] = await db.select().from(configuration).where(eq(configuration.key, key)).limit(1)
    if (!row) return null
    return row.value as Record<string, unknown>
}

/**
 * Upsert a configuration value by key.
 */
export async function setConfigValue(key: string, value: Record<string, unknown>): Promise<void> {
    await db
        .insert(configuration)
        .values({ key, value })
        .onConflictDoUpdate({
            target: configuration.key,
            set: { value, updatedAt: new Date() },
        })
}
