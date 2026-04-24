import { eq } from 'drizzle-orm'
import { db } from './index'
import { serviceCredentials, type ServiceCredentials } from './schema'
import { encryptConfig } from '$lib/server/crypto/encryption'
import { ulid } from 'ulid'

export class ServiceCredentialsRepo {
    static async getBySourceId(sourceId: string): Promise<ServiceCredentials | undefined> {
        return await db.query.serviceCredentials.findFirst({
            where: eq(serviceCredentials.sourceId, sourceId),
        })
    }

    static async create(data: {
        sourceId: string
        provider: string
        authType: string
        principalEmail: string | null
        credentials: Record<string, unknown>
        config: Record<string, unknown>
    }): Promise<ServiceCredentials> {
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, data.sourceId))

        const [created] = await db
            .insert(serviceCredentials)
            .values({
                id: ulid(),
                sourceId: data.sourceId,
                provider: data.provider,
                authType: data.authType,
                principalEmail: data.principalEmail,
                credentials: encryptConfig(data.credentials),
                config: data.config,
            })
            .returning()

        return created
    }

    static async updateBySourceId(
        sourceId: string,
        data: {
            principalEmail?: string | null
            credentials?: Record<string, unknown> | null
            config?: Record<string, unknown>
        },
    ): Promise<ServiceCredentials | undefined> {
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
            .where(eq(serviceCredentials.sourceId, sourceId))
            .returning()

        return updated
    }

    static async deleteBySourceId(sourceId: string): Promise<void> {
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))
    }
}
