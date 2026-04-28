import { and, eq, sql } from 'drizzle-orm'
import { db } from './index'
import { userPreferences } from './schema'

export class UserPreferencesRepository {
    async get<T = unknown>(userId: string, key: string): Promise<T | null> {
        const [row] = await db
            .select({ value: userPreferences.value })
            .from(userPreferences)
            .where(and(eq(userPreferences.userId, userId), eq(userPreferences.key, key)))
            .limit(1)
        return (row?.value as T | undefined) ?? null
    }

    async set(userId: string, key: string, value: unknown): Promise<void> {
        await db
            .insert(userPreferences)
            .values({ userId, key, value })
            .onConflictDoUpdate({
                target: [userPreferences.userId, userPreferences.key],
                set: { value, updatedAt: sql`NOW()` },
            })
    }

    async delete(userId: string, key: string): Promise<void> {
        await db
            .delete(userPreferences)
            .where(and(eq(userPreferences.userId, userId), eq(userPreferences.key, key)))
    }
}

export const userPreferencesRepository = new UserPreferencesRepository()
