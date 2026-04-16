import { eq, and, desc } from 'drizzle-orm'
import { sha256 } from '@oslojs/crypto/sha2'
import { encodeBase64url, encodeHexLowerCase } from '@oslojs/encoding'
import { timingSafeEqual } from 'crypto'
import { db } from '$lib/server/db'
import * as table from '$lib/server/db/schema'
import { ulid } from 'ulid'
import { createLogger } from '$lib/server/logger.js'

const API_KEY_PREFIX = 'omni_'
const MAX_KEYS_PER_USER = 25
const logger = createLogger('api-keys')

export function generateApiKey(): { key: string; hash: string; prefix: string } {
    const bytes = crypto.getRandomValues(new Uint8Array(32))
    const randomPart = encodeBase64url(bytes)
    const key = `${API_KEY_PREFIX}${randomPart}`
    const hash = hashApiKey(key)
    const prefix = key.substring(0, 12)
    return { key, hash, prefix }
}

export function hashApiKey(key: string): string {
    return encodeHexLowerCase(sha256(new TextEncoder().encode(key)))
}

export async function validateApiKey(
    key: string,
): Promise<{
    user: Pick<
        typeof table.user.$inferSelect,
        'id' | 'email' | 'role' | 'isActive' | 'mustChangePassword'
    >
    allowedSources: string[] | null
    scope: 'public' | 'user' | 'admin'
} | null> {
    const hash = hashApiKey(key)

    const [result] = await db
        .select({
            apiKey: table.apiKeys,
            user: {
                id: table.user.id,
                email: table.user.email,
                role: table.user.role,
                isActive: table.user.isActive,
                mustChangePassword: table.user.mustChangePassword,
            },
        })
        .from(table.apiKeys)
        .innerJoin(table.user, eq(table.apiKeys.userId, table.user.id))
        .where(and(eq(table.apiKeys.keyHash, hash), eq(table.apiKeys.isActive, true)))
        .limit(1)

    if (!result) return null

    // Timing-safe comparison as defense-in-depth
    const incomingBuf = Buffer.from(hash, 'hex')
    const storedBuf = Buffer.from(result.apiKey.keyHash.trim(), 'hex')
    if (incomingBuf.length !== storedBuf.length || !timingSafeEqual(incomingBuf, storedBuf)) {
        return null
    }

    // Check expiry
    if (result.apiKey.expiresAt && result.apiKey.expiresAt < new Date()) {
        return null
    }

    // Check user is active
    if (!result.user.isActive) return null

    // Update last_used_at (fire-and-forget)
    db.update(table.apiKeys)
        .set({ lastUsedAt: new Date(), updatedAt: new Date() })
        .where(eq(table.apiKeys.id, result.apiKey.id))
        .then(() => {})
        .catch((err: Error) => {
            logger.warn('Failed to update api key last_used_at', { error: err.message })
        })

    const allowedSources = result.apiKey.allowedSources as string[] | null
    const rawScope = result.apiKey.scope
    const scope = (rawScope === 'admin' ? 'admin' : rawScope === 'user' ? 'user' : 'public') as
        | 'public'
        | 'user'
        | 'admin'
    return { user: result.user, allowedSources, scope }
}

export async function createApiKey(
    userId: string,
    name: string,
    expiresAt?: Date,
    allowedSources?: string[] | null,
    scope?: 'public' | 'user' | 'admin',
): Promise<{ id: string; key: string; prefix: string }> {
    // Check per-user key count limit
    const existing = await db
        .select({ id: table.apiKeys.id })
        .from(table.apiKeys)
        .where(and(eq(table.apiKeys.userId, userId), eq(table.apiKeys.isActive, true)))

    if (existing.length >= MAX_KEYS_PER_USER) {
        throw new Error(`Maximum of ${MAX_KEYS_PER_USER} active API keys per user`)
    }

    const { key, hash, prefix } = generateApiKey()
    const id = ulid()

    await db.insert(table.apiKeys).values({
        id,
        userId,
        keyHash: hash,
        keyPrefix: prefix,
        name,
        expiresAt: expiresAt ?? null,
        allowedSources: allowedSources ?? null,
        scope: scope ?? 'public',
    })

    return { id, key, prefix }
}

export async function listApiKeys(userId: string) {
    return db
        .select({
            id: table.apiKeys.id,
            name: table.apiKeys.name,
            keyPrefix: table.apiKeys.keyPrefix,
            allowedSources: table.apiKeys.allowedSources,
            scope: table.apiKeys.scope,
            lastUsedAt: table.apiKeys.lastUsedAt,
            expiresAt: table.apiKeys.expiresAt,
            isActive: table.apiKeys.isActive,
            createdAt: table.apiKeys.createdAt,
        })
        .from(table.apiKeys)
        .where(eq(table.apiKeys.userId, userId))
        .orderBy(desc(table.apiKeys.createdAt))
}

export async function revokeApiKey(keyId: string, userId: string): Promise<boolean> {
    const result = await db
        .update(table.apiKeys)
        .set({ isActive: false, updatedAt: new Date() })
        .where(and(eq(table.apiKeys.id, keyId), eq(table.apiKeys.userId, userId)))
        .returning({ id: table.apiKeys.id })

    return result.length > 0
}

export async function deleteApiKey(keyId: string, userId: string): Promise<boolean> {
    const result = await db
        .delete(table.apiKeys)
        .where(and(eq(table.apiKeys.id, keyId), eq(table.apiKeys.userId, userId)))
        .returning({ id: table.apiKeys.id })

    return result.length > 0
}
