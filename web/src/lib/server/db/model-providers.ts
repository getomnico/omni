import { eq, and } from 'drizzle-orm'
import { db } from './index'
import { modelProviders, models } from './schema'
import type { ModelProvider, Model } from './schema'
import { ulid } from 'ulid'

export const MODEL_PROVIDER_TYPES = ['vllm', 'anthropic', 'bedrock', 'openai'] as const
export type ModelProviderType = (typeof MODEL_PROVIDER_TYPES)[number]

export interface ModelProviderConfig {
    apiKey?: string | null
    apiUrl?: string | null
    regionName?: string | null
}

export interface CreateProviderInput {
    name: string
    providerType: ModelProviderType
    config: ModelProviderConfig
}

export interface UpdateProviderInput {
    name?: string
    config?: ModelProviderConfig
}

export interface CreateModelInput {
    modelProviderId: string
    modelId: string
    displayName: string
    isDefault?: boolean
}

export const PREDEFINED_MODELS: Record<
    ModelProviderType,
    { modelId: string; displayName: string }[]
> = {
    anthropic: [
        { modelId: 'claude-opus-4-6', displayName: 'Claude Opus 4.6' },
        { modelId: 'claude-sonnet-4-5-20250929', displayName: 'Claude Sonnet 4.5' },
        { modelId: 'claude-haiku-4-5-20251001', displayName: 'Claude Haiku 4.5' },
    ],
    openai: [
        { modelId: 'gpt-5.2', displayName: 'GPT-5.2' },
        { modelId: 'gpt-5-mini', displayName: 'GPT-5 Mini' },
        { modelId: 'gpt-4.1', displayName: 'GPT-4.1' },
    ],
    bedrock: [
        { modelId: 'anthropic.claude-opus-4-6-v1', displayName: 'Claude Opus 4.6' },
        { modelId: 'anthropic.claude-sonnet-4-5-20250929-v1:0', displayName: 'Claude Sonnet 4.5' },
        { modelId: 'anthropic.claude-haiku-4-5-20251001-v1:0', displayName: 'Claude Haiku 4.5' },
        { modelId: 'amazon.nova-pro-v1:0', displayName: 'Amazon Nova Pro' },
    ],
    vllm: [],
}

// --- Provider CRUD ---

export async function listActiveProviders(): Promise<ModelProvider[]> {
    return await db
        .select()
        .from(modelProviders)
        .where(eq(modelProviders.isDeleted, false))
        .orderBy(modelProviders.createdAt)
}

export async function getProvider(id: string): Promise<ModelProvider | null> {
    const [provider] = await db
        .select()
        .from(modelProviders)
        .where(eq(modelProviders.id, id))
        .limit(1)
    return provider || null
}

export async function createProvider(input: CreateProviderInput): Promise<ModelProvider> {
    const [provider] = await db
        .insert(modelProviders)
        .values({
            id: ulid(),
            name: input.name,
            providerType: input.providerType,
            config: input.config,
        })
        .returning()

    return provider
}

export async function updateProvider(
    id: string,
    input: UpdateProviderInput,
): Promise<ModelProvider | null> {
    const values: Record<string, unknown> = { updatedAt: new Date() }
    if (input.name !== undefined) values.name = input.name
    if (input.config !== undefined) values.config = input.config

    const [updated] = await db
        .update(modelProviders)
        .set(values)
        .where(eq(modelProviders.id, id))
        .returning()

    return updated || null
}

export async function deleteProvider(id: string): Promise<boolean> {
    const [updated] = await db
        .update(modelProviders)
        .set({ isDeleted: true, updatedAt: new Date() })
        .where(eq(modelProviders.id, id))
        .returning()

    return !!updated
}

// --- Model CRUD ---

export async function listModelsByProvider(providerId: string): Promise<Model[]> {
    return await db
        .select()
        .from(models)
        .where(and(eq(models.modelProviderId, providerId), eq(models.isDeleted, false)))
        .orderBy(models.createdAt)
}

export async function listAllActiveModels(): Promise<
    (Model & { providerType: string; providerName: string })[]
> {
    const rows = await db
        .select({
            id: models.id,
            modelProviderId: models.modelProviderId,
            modelId: models.modelId,
            displayName: models.displayName,
            isDefault: models.isDefault,
            isDeleted: models.isDeleted,
            createdAt: models.createdAt,
            updatedAt: models.updatedAt,
            providerType: modelProviders.providerType,
            providerName: modelProviders.name,
        })
        .from(models)
        .innerJoin(modelProviders, eq(models.modelProviderId, modelProviders.id))
        .where(and(eq(models.isDeleted, false), eq(modelProviders.isDeleted, false)))
        .orderBy(models.createdAt)

    return rows
}

export async function getModel(id: string): Promise<Model | null> {
    const [model] = await db.select().from(models).where(eq(models.id, id)).limit(1)
    return model || null
}

export async function createModel(input: CreateModelInput): Promise<Model> {
    if (input.isDefault) {
        await db
            .update(models)
            .set({ isDefault: false, updatedAt: new Date() })
            .where(eq(models.isDefault, true))
    }

    const [model] = await db
        .insert(models)
        .values({
            id: ulid(),
            modelProviderId: input.modelProviderId,
            modelId: input.modelId,
            displayName: input.displayName,
            isDefault: input.isDefault ?? false,
        })
        .returning()

    return model
}

export async function deleteModel(id: string): Promise<boolean> {
    const [updated] = await db
        .update(models)
        .set({ isDeleted: true, isDefault: false, updatedAt: new Date() })
        .where(eq(models.id, id))
        .returning()

    return !!updated
}

export async function setDefaultModel(id: string): Promise<boolean> {
    await db
        .update(models)
        .set({ isDefault: false, updatedAt: new Date() })
        .where(eq(models.isDefault, true))

    const [updated] = await db
        .update(models)
        .set({ isDefault: true, updatedAt: new Date() })
        .where(and(eq(models.id, id), eq(models.isDeleted, false)))
        .returning()

    return !!updated
}

export async function createPredefinedModels(
    providerId: string,
    providerType: ModelProviderType,
): Promise<Model[]> {
    const predefined = PREDEFINED_MODELS[providerType]
    if (!predefined || predefined.length === 0) return []

    const hasExistingDefault = await db
        .select({ id: models.id })
        .from(models)
        .where(and(eq(models.isDefault, true), eq(models.isDeleted, false)))
        .limit(1)

    const createdModels: Model[] = []
    for (let i = 0; i < predefined.length; i++) {
        const isDefault = i === 0 && hasExistingDefault.length === 0
        const model = await createModel({
            modelProviderId: providerId,
            modelId: predefined[i].modelId,
            displayName: predefined[i].displayName,
            isDefault,
        })
        createdModels.push(model)
    }

    return createdModels
}
