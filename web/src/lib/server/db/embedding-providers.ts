import { eq, and, sql } from 'drizzle-orm'
import { db } from './index'
import { embeddingProviders } from './schema'
import type { EmbeddingProvider } from './schema'
import { ulid } from 'ulid'
import { encryptConfig, decryptConfig } from '$lib/server/crypto/encryption'

export { EMBEDDING_PROVIDER_TYPES, type EmbeddingProviderType, PROVIDER_LABELS } from '$lib/types'
import { type EmbeddingProviderType } from '$lib/types'

export interface EmbeddingProviderConfig {
    apiKey?: string | null
    apiUrl?: string | null
    model?: string | null
    dimensions?: number | null
    maxModelLen?: number | null
}

export interface CreateEmbeddingProviderInput {
    name: string
    providerType: EmbeddingProviderType
    config: EmbeddingProviderConfig
}

export interface UpdateEmbeddingProviderInput {
    name?: string
    config?: EmbeddingProviderConfig
}

export async function listActiveProviders(): Promise<EmbeddingProvider[]> {
    const rows = await db
        .select()
        .from(embeddingProviders)
        .where(eq(embeddingProviders.isDeleted, false))
        .orderBy(embeddingProviders.createdAt)
    return rows.map((row) => ({ ...row, config: decryptConfig(row.config) }))
}

export async function getProvider(id: string): Promise<EmbeddingProvider | null> {
    const [provider] = await db
        .select()
        .from(embeddingProviders)
        .where(eq(embeddingProviders.id, id))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function getCurrentProvider(): Promise<EmbeddingProvider | null> {
    const [provider] = await db
        .select()
        .from(embeddingProviders)
        .where(and(eq(embeddingProviders.isCurrent, true), eq(embeddingProviders.isDeleted, false)))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function createProvider(
    input: CreateEmbeddingProviderInput,
): Promise<EmbeddingProvider> {
    const existing = await getCurrentProvider()
    const shouldBeCurrent = !existing

    const [provider] = await db
        .insert(embeddingProviders)
        .values({
            id: ulid(),
            name: input.name,
            providerType: input.providerType,
            config: encryptConfig(input.config as Record<string, unknown>),
            isCurrent: shouldBeCurrent,
        })
        .returning()

    return { ...provider, config: decryptConfig(provider.config) }
}

export async function updateProvider(
    id: string,
    input: UpdateEmbeddingProviderInput,
): Promise<EmbeddingProvider | null> {
    const values: Record<string, unknown> = { updatedAt: new Date() }
    if (input.name !== undefined) values.name = input.name
    if (input.config !== undefined)
        values.config = encryptConfig(input.config as Record<string, unknown>)

    const [updated] = await db
        .update(embeddingProviders)
        .set(values)
        .where(eq(embeddingProviders.id, id))
        .returning()

    if (!updated) return null
    return { ...updated, config: decryptConfig(updated.config) }
}

export async function deleteProvider(id: string): Promise<boolean> {
    const [updated] = await db
        .update(embeddingProviders)
        .set({ isDeleted: true, isCurrent: false, updatedAt: new Date() })
        .where(eq(embeddingProviders.id, id))
        .returning()

    return !!updated
}

export async function setCurrentProvider(
    id: string,
): Promise<{ previous: EmbeddingProvider | null }> {
    const previous = await getCurrentProvider()

    // Clear old current
    await db
        .update(embeddingProviders)
        .set({ isCurrent: false, updatedAt: new Date() })
        .where(eq(embeddingProviders.isCurrent, true))

    // Set new current
    await db
        .update(embeddingProviders)
        .set({ isCurrent: true, updatedAt: new Date() })
        .where(and(eq(embeddingProviders.id, id), eq(embeddingProviders.isDeleted, false)))

    return { previous }
}

export { type EmbeddingProvider }
