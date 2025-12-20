import { eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type LLMProvider = 'vllm' | 'anthropic' | 'bedrock'

// Shared generation parameters (all optional with sensible defaults)
interface LLMGenerationParams {
    maxTokens: number | null
    temperature: number | null
    topP: number | null
}

// Provider-specific config types
export interface VLLMConfig extends LLMGenerationParams {
    provider: 'vllm'
    vllmUrl: string
    primaryModelId: string | null
    secondaryModelId: string | null
}

export interface AnthropicConfig extends LLMGenerationParams {
    provider: 'anthropic'
    anthropicApiKey: string
    primaryModelId: string
    secondaryModelId: string | null
}

export interface BedrockLLMConfig extends LLMGenerationParams {
    provider: 'bedrock'
    primaryModelId: string
    secondaryModelId: string | null
}

// Discriminated union type
export type LLMConfig = VLLMConfig | AnthropicConfig | BedrockLLMConfig

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

    const generationParams: LLMGenerationParams = {
        maxTokens: raw.maxTokens as number | null,
        temperature: raw.temperature as number | null,
        topP: raw.topP as number | null,
    }

    switch (provider) {
        case 'vllm':
            return {
                provider: 'vllm',
                vllmUrl: raw.vllmUrl as string,
                primaryModelId: raw.primaryModelId as string | null,
                secondaryModelId: raw.secondaryModelId as string | null,
                ...generationParams,
            }
        case 'anthropic':
            return {
                provider: 'anthropic',
                anthropicApiKey: raw.anthropicApiKey as string,
                primaryModelId: raw.primaryModelId as string,
                secondaryModelId: raw.secondaryModelId as string | null,
                ...generationParams,
            }
        case 'bedrock':
            return {
                provider: 'bedrock',
                primaryModelId: raw.primaryModelId as string,
                secondaryModelId: raw.secondaryModelId as string | null,
                ...generationParams,
            }
        default:
            console.warn(`Unknown LLM provider: ${provider}`)
            return null
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
