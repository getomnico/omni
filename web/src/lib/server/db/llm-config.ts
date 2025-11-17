import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type LLMProvider = 'vllm' | 'anthropic' | 'bedrock'

export interface LLMConfig {
    provider: LLMProvider
    primaryModelId: string
    secondaryModelId?: string | null
    vllmUrl?: string | null
    anthropicApiKey?: string | null
    maxTokens?: number | null
    temperature?: number | null
    topP?: number | null
}

export interface LLMConfigData {
    provider: LLMProvider
    primaryModelId: string
    secondaryModelId?: string | null
    vllmUrl?: string | null
    anthropicApiKey?: string | null
    maxTokens?: number | null
    temperature?: number | null
    topP?: number | null
}

const LLM_CONFIG_KEY = 'llm_config'

export async function getLLMConfig(): Promise<LLMConfig | null> {
    const result = await db
        .select()
        .from(configuration)
        .where(eq(configuration.key, LLM_CONFIG_KEY))
        .limit(1)

    if (result.length === 0) {
        return null
    }

    return result[0].value as LLMConfig
}

export async function upsertLLMConfig(data: LLMConfigData): Promise<void> {
    const existing = await getLLMConfig()

    const configValue: LLMConfig = {
        provider: data.provider,
        primaryModelId: data.primaryModelId,
        secondaryModelId: data.secondaryModelId || null,
        vllmUrl: data.vllmUrl || null,
        anthropicApiKey: data.anthropicApiKey || null,
        maxTokens: data.maxTokens || null,
        temperature: data.temperature || null,
        topP: data.topP || null,
    }

    if (existing) {
        await db
            .update(configuration)
            .set({
                value: configValue,
                updatedAt: new Date(),
            })
            .where(eq(configuration.key, LLM_CONFIG_KEY))
    } else {
        await db.insert(configuration).values({
            key: LLM_CONFIG_KEY,
            value: configValue,
        })
    }
}

export async function deleteLLMConfig(): Promise<void> {
    await db.delete(configuration).where(eq(configuration.key, LLM_CONFIG_KEY))
}
