import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type LLMProvider = 'vllm' | 'anthropic' | 'bedrock'

export interface LLMConfig {
    provider: LLMProvider
    apiKey: string | null
    model: string
    apiUrl: string | null
    secondaryModel: string | null
    maxTokens: number | null
    temperature: number | null
    topP: number | null
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

    const raw = result[0].value as Record<string, unknown>
    const provider = raw.provider as LLMProvider

    if (!['vllm', 'anthropic', 'bedrock'].includes(provider)) {
        console.warn(`Unknown LLM provider: ${provider}`)
        return null
    }

    return {
        provider,
        apiKey: (raw.apiKey as string) ?? null,
        model: (raw.model as string) ?? '',
        apiUrl: (raw.apiUrl as string) ?? null,
        secondaryModel: (raw.secondaryModel as string) ?? null,
        maxTokens: (raw.maxTokens as number) ?? null,
        temperature: (raw.temperature as number) ?? null,
        topP: (raw.topP as number) ?? null,
    }
}

export async function upsertLLMConfig(data: LLMConfig): Promise<void> {
    const existing = await db
        .select()
        .from(configuration)
        .where(eq(configuration.key, LLM_CONFIG_KEY))
        .limit(1)

    if (existing.length > 0) {
        await db
            .update(configuration)
            .set({
                value: data,
                updatedAt: new Date(),
            })
            .where(eq(configuration.key, LLM_CONFIG_KEY))
    } else {
        await db.insert(configuration).values({
            key: LLM_CONFIG_KEY,
            value: data,
        })
    }
}

export async function deleteLLMConfig(): Promise<void> {
    await db.delete(configuration).where(eq(configuration.key, LLM_CONFIG_KEY))
}
