import { and, eq, isNull, isNotNull } from 'drizzle-orm'
import { db } from '$lib/server/db'
import { serviceCredentials, type ServiceCredential } from '$lib/server/db/schema'
import { encryptConfig } from '$lib/server/crypto/encryption'
import { ulid } from 'ulid'

/// Public view of a per-user credential row used by admin/source UIs to list
/// "who has connected" without exposing secret material.
export type UserCredentialSummary = Pick<
    ServiceCredential,
    'userId' | 'principalEmail' | 'createdAt'
> & { userId: string }

export class ServiceCredentialsRepository {
    /// Fetch the org-wide credential (`user_id IS NULL`) for a source. Org-wide
    /// rows back sync and serve as the read fallback for org-scoped sources.
    async getOrgCredsBySourceId(sourceId: string): Promise<ServiceCredential | null> {
        const result = await db.query.serviceCredentials.findFirst({
            where: and(
                eq(serviceCredentials.sourceId, sourceId),
                isNull(serviceCredentials.userId),
            ),
        })
        return result ?? null
    }

    /// Fetch a per-user credential for an org-wide source.
    async getByUserAndSource(sourceId: string, userId: string): Promise<ServiceCredential | null> {
        const result = await db.query.serviceCredentials.findFirst({
            where: and(
                eq(serviceCredentials.sourceId, sourceId),
                eq(serviceCredentials.userId, userId),
            ),
        })
        return result ?? null
    }

    /// List per-user credential rows for a source — used by admin UI to show
    /// who has connected. Strips secret material.
    async listUserCredentialsForSource(sourceId: string): Promise<UserCredentialSummary[]> {
        const rows = await db
            .select({
                userId: serviceCredentials.userId,
                principalEmail: serviceCredentials.principalEmail,
                createdAt: serviceCredentials.createdAt,
            })
            .from(serviceCredentials)
            .where(
                and(
                    eq(serviceCredentials.sourceId, sourceId),
                    isNotNull(serviceCredentials.userId),
                ),
            )
        return rows
            .filter((r): r is UserCredentialSummary => r.userId !== null)
            .map((r) => ({
                userId: r.userId,
                principalEmail: r.principalEmail,
                createdAt: r.createdAt,
            }))
    }

    /// Upsert the org-wide credential for a source. Replaces any existing org-wide
    /// row but leaves per-user rows alone (they remain valid against the same
    /// source even if the org credential rotates).
    async create(data: {
        sourceId: string
        provider: string
        authType: string
        principalEmail: string | null
        credentials: Record<string, unknown>
        config: Record<string, unknown>
    }): Promise<ServiceCredential> {
        await db
            .delete(serviceCredentials)
            .where(
                and(
                    eq(serviceCredentials.sourceId, data.sourceId),
                    isNull(serviceCredentials.userId),
                ),
            )

        const [created] = await db
            .insert(serviceCredentials)
            .values({
                id: ulid(),
                sourceId: data.sourceId,
                userId: null,
                provider: data.provider,
                authType: data.authType,
                principalEmail: data.principalEmail,
                credentials: encryptConfig(data.credentials),
                config: data.config,
            })
            .returning()

        return created
    }

    /// Upsert a per-user credential for an org-wide source. Replaces any existing
    /// per-user row for the same (source, user). Used by the per-user OAuth
    /// callback (`/api/sources/[sourceId]/user-auth/callback`).
    async createForUser(data: {
        sourceId: string
        userId: string
        provider: string
        authType: string
        principalEmail: string | null
        credentials: Record<string, unknown>
        config: Record<string, unknown>
        expiresAt?: Date | null
    }): Promise<ServiceCredential> {
        await db
            .delete(serviceCredentials)
            .where(
                and(
                    eq(serviceCredentials.sourceId, data.sourceId),
                    eq(serviceCredentials.userId, data.userId),
                ),
            )

        const [created] = await db
            .insert(serviceCredentials)
            .values({
                id: ulid(),
                sourceId: data.sourceId,
                userId: data.userId,
                provider: data.provider,
                authType: data.authType,
                principalEmail: data.principalEmail,
                credentials: encryptConfig(data.credentials),
                config: data.config,
                expiresAt: data.expiresAt ?? null,
            })
            .returning()

        return created
    }

    async updateBySourceId(
        sourceId: string,
        data: {
            principalEmail?: string | null
            credentials?: Record<string, unknown> | null
            config?: Record<string, unknown>
        },
    ): Promise<ServiceCredential | null> {
        const updates: Partial<typeof serviceCredentials.$inferInsert> = {
            updatedAt: new Date(),
        }

        if (data.principalEmail !== undefined) {
            updates.principalEmail = data.principalEmail
        }
        if (data.config !== undefined) {
            updates.config = data.config
        }
        if (data.credentials) {
            updates.credentials = encryptConfig(data.credentials)
        }

        const [updated] = await db
            .update(serviceCredentials)
            .set(updates)
            .where(
                and(eq(serviceCredentials.sourceId, sourceId), isNull(serviceCredentials.userId)),
            )
            .returning()

        return updated ?? null
    }

    /// Delete every credential row for a source — used when the source itself is
    /// being removed.
    async deleteBySourceId(sourceId: string): Promise<void> {
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))
    }

    /// Delete a single per-user credential — used by the user-side disconnect
    /// action in settings.
    async deleteForUser(sourceId: string, userId: string): Promise<void> {
        await db
            .delete(serviceCredentials)
            .where(
                and(
                    eq(serviceCredentials.sourceId, sourceId),
                    eq(serviceCredentials.userId, userId),
                ),
            )
    }
}

export const serviceCredentialsRepository = new ServiceCredentialsRepository()
