import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type EmbeddingProvider = 'jina' | 'bedrock'

export interface EmbeddingConfig {
    provider: EmbeddingProvider
    jinaApiKey?: string | null
    jinaModel?: string | null
    jinaApiUrl?: string | null
    bedrockModelId?: string | null
}

export interface EmbeddingConfigData {
    provider: EmbeddingProvider
    jinaApiKey?: string | null
    jinaModel?: string | null
    jinaApiUrl?: string | null
    bedrockModelId?: string | null
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

    return result[0].value as EmbeddingConfig
}

export async function upsertEmbeddingConfig(data: EmbeddingConfigData): Promise<void> {
    const existing = await getEmbeddingConfig()

    const configValue: EmbeddingConfig = {
        provider: data.provider,
        jinaApiKey: data.jinaApiKey || null,
        jinaModel: data.jinaModel || null,
        jinaApiUrl: data.jinaApiUrl || null,
        bedrockModelId: data.bedrockModelId || null,
    }

    if (existing) {
        await db
            .update(configuration)
            .set({
                value: configValue,
                updatedAt: new Date(),
            })
            .where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
    } else {
        await db.insert(configuration).values({
            key: EMBEDDING_CONFIG_KEY,
            value: configValue,
        })
    }
}

export async function deleteEmbeddingConfig(): Promise<void> {
    await db.delete(configuration).where(eq(configuration.key, EMBEDDING_CONFIG_KEY))
}
