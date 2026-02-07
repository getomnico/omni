import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type EmbeddingProvider = 'local' | 'jina' | 'openai' | 'cohere' | 'bedrock'

export interface EmbeddingConfig {
    provider: EmbeddingProvider
    apiKey: string | null
    model: string
    apiUrl: string | null
    dimensions: number | null
    maxModelLen: number | null
}

const EMBEDDING_CONFIG_KEY = 'embedding_config'

export async function getEmbeddingConfig(): Promise<EmbeddingConfig | null> {
    const result = await db
        .select()
        .from(configuration)
        .where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
        .limit(1)

    if (result.length === 0) {
        return null
    }

    const raw = result[0].value as Record<string, unknown>
    const provider = raw.provider as EmbeddingProvider

    if (!['local', 'jina', 'openai', 'cohere', 'bedrock'].includes(provider)) {
        console.warn(`Unknown embedding provider: ${provider}`)
        return null
    }

    return {
        provider,
        apiKey: (raw.apiKey as string) ?? null,
        model: (raw.model as string) ?? '',
        apiUrl: (raw.apiUrl as string) ?? null,
        dimensions: (raw.dimensions as number) ?? null,
        maxModelLen: (raw.maxModelLen as number) ?? null,
    }
}

export async function upsertEmbeddingConfig(data: EmbeddingConfig): Promise<void> {
    const existing = await db
        .select()
        .from(configuration)
        .where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
        .limit(1)

    if (existing.length > 0) {
        await db
            .update(configuration)
            .set({
                value: data,
                updatedAt: new Date(),
            })
            .where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
    } else {
        await db.insert(configuration).values({
            key: EMBEDDING_CONFIG_KEY,
            value: data,
        })
    }
}

export async function deleteEmbeddingConfig(): Promise<void> {
    await db.delete(configuration).where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
}
