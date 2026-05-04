import { and, eq, sql } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type ConfigurationValue = Record<string, unknown>

/**
 * Get a global-scope configuration value by key. Returns null if not found.
 */
export async function getGlobal(key: string): Promise<ConfigurationValue | null> {
    const [row] = await db
        .select({ value: configuration.value })
        .from(configuration)
        .where(and(eq(configuration.scope, 'global'), eq(configuration.key, key)))
        .limit(1)
    return (row?.value as ConfigurationValue | undefined) ?? null
}

/**
 * Get a per-user configuration value. Returns null if not found.
 */
export async function getUser(userId: string, key: string): Promise<ConfigurationValue | null> {
    const [row] = await db
        .select({ value: configuration.value })
        .from(configuration)
        .where(
            and(
                eq(configuration.scope, 'user'),
                eq(configuration.userId, userId),
                eq(configuration.key, key),
            ),
        )
        .limit(1)
    return (row?.value as ConfigurationValue | undefined) ?? null
}

/**
 * Upsert a global-scope configuration value.
 */
export async function setGlobal(key: string, value: ConfigurationValue): Promise<void> {
    // The unique index is partial (`WHERE scope = 'global'`), which Drizzle's
    // `onConflictDoUpdate` doesn't model directly — fall back to raw SQL.
    const json = JSON.stringify(value)
    await db.execute(sql`
        INSERT INTO configuration (scope, user_id, key, value)
        VALUES ('global', NULL, ${key}, ${json}::jsonb)
        ON CONFLICT (key) WHERE scope = 'global'
        DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()
    `)
}

/**
 * Upsert a per-user configuration value.
 */
export async function setUser(
    userId: string,
    key: string,
    value: ConfigurationValue,
): Promise<void> {
    const json = JSON.stringify(value)
    await db.execute(sql`
        INSERT INTO configuration (scope, user_id, key, value)
        VALUES ('user', ${userId}, ${key}, ${json}::jsonb)
        ON CONFLICT (user_id, key) WHERE scope = 'user'
        DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()
    `)
}

/**
 * Delete a per-user configuration value, falling back to the global default.
 */
export async function deleteUser(userId: string, key: string): Promise<void> {
    await db
        .delete(configuration)
        .where(
            and(
                eq(configuration.scope, 'user'),
                eq(configuration.userId, userId),
                eq(configuration.key, key),
            ),
        )
}
