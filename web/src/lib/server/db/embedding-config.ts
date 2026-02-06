import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type EmbeddingProvider = 'local' | 'jina' | 'openai' | 'cohere' | 'bedrock'

// Provider-specific config types
export interface LocalEmbeddingConfig {
    provider: 'local'
    localBaseUrl: string
    localModel: string
}

export interface JinaEmbeddingConfig {
    provider: 'jina'
    jinaApiKey: string | null
    jinaModel: string
    jinaApiUrl: string | null
}

export interface OpenAIEmbeddingConfig {
    provider: 'openai'
    openaiApiKey: string | null
    openaiModel: string
    openaiDimensions: number | null
}

export interface CohereEmbeddingConfig {
    provider: 'cohere'
    cohereApiKey: string | null
    cohereModel: string
    cohereApiUrl: string | null
    cohereDimensions: number | null
}

export interface BedrockEmbeddingConfig {
    provider: 'bedrock'
    bedrockModelId: string
}

// Discriminated union type
export type EmbeddingConfig =
    | LocalEmbeddingConfig
    | JinaEmbeddingConfig
    | OpenAIEmbeddingConfig
    | CohereEmbeddingConfig
    | BedrockEmbeddingConfig

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

    switch (provider) {
        case 'local':
            return {
                provider: 'local',
                localBaseUrl: raw.localBaseUrl as string,
                localModel: raw.localModel as string,
            }
        case 'jina':
            return {
                provider: 'jina',
                jinaApiKey: raw.jinaApiKey as string | null,
                jinaModel: raw.jinaModel as string,
                jinaApiUrl: raw.jinaApiUrl as string | null,
            }
        case 'openai':
            return {
                provider: 'openai',
                openaiApiKey: raw.openaiApiKey as string | null,
                openaiModel: raw.openaiModel as string,
                openaiDimensions: raw.openaiDimensions as number | null,
            }
        case 'cohere':
            return {
                provider: 'cohere',
                cohereApiKey: raw.cohereApiKey as string | null,
                cohereModel: raw.cohereModel as string,
                cohereApiUrl: raw.cohereApiUrl as string | null,
                cohereDimensions: raw.cohereDimensions as number | null,
            }
        case 'bedrock':
            return {
                provider: 'bedrock',
                bedrockModelId: raw.bedrockModelId as string,
            }
        default:
            console.warn(`Unknown embedding provider: ${provider}`)
            return null
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
