import { eq, and } from 'drizzle-orm'
import { db } from './index'
import { webSearchProviders } from './schema'
import type { WebSearchProvider } from './schema'
import { ulid } from 'ulid'
import { encryptConfig, decryptConfig } from '$lib/server/crypto/encryption'

export {
    WEB_SEARCH_PROVIDER_TYPES,
    WEB_SEARCH_PROVIDER_LABELS,
    type WebSearchProviderType,
} from '$lib/types'
import type { WebSearchProviderType } from '$lib/types'

export interface WebSearchProviderConfig {
    apiKey?: string | null
    baseUrl?: string | null
    engineId?: string | null
}

export interface CreateWebSearchProviderInput {
    name: string
    providerType: WebSearchProviderType
    config: WebSearchProviderConfig
}

export interface UpdateWebSearchProviderInput {
    name?: string
    config?: WebSearchProviderConfig
}

export async function listActiveProviders(): Promise<WebSearchProvider[]> {
    const rows = await db
        .select()
        .from(webSearchProviders)
        .where(eq(webSearchProviders.isDeleted, false))
        .orderBy(webSearchProviders.createdAt)
    return rows.map((row) => ({ ...row, config: decryptConfig(row.config) }))
}

export async function getProvider(id: string): Promise<WebSearchProvider | null> {
    const [provider] = await db
        .select()
        .from(webSearchProviders)
        .where(eq(webSearchProviders.id, id))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function getCurrentProvider(): Promise<WebSearchProvider | null> {
    const [provider] = await db
        .select()
        .from(webSearchProviders)
        .where(and(eq(webSearchProviders.isCurrent, true), eq(webSearchProviders.isDeleted, false)))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function createProvider(
    input: CreateWebSearchProviderInput,
): Promise<WebSearchProvider> {
    const existing = await getCurrentProvider()
    const [provider] = await db
        .insert(webSearchProviders)
        .values({
            id: ulid(),
            name: input.name,
            providerType: input.providerType,
            config: encryptConfig(input.config as Record<string, unknown>),
            isCurrent: !existing,
        })
        .returning()

    return { ...provider, config: decryptConfig(provider.config) }
}

export async function updateProvider(
    id: string,
    input: UpdateWebSearchProviderInput,
): Promise<WebSearchProvider | null> {
    const values: Record<string, unknown> = { updatedAt: new Date() }
    if (input.name !== undefined) values.name = input.name
    if (input.config !== undefined) {
        values.config = encryptConfig(input.config as Record<string, unknown>)
    }

    const [updated] = await db
        .update(webSearchProviders)
        .set(values)
        .where(eq(webSearchProviders.id, id))
        .returning()

    if (!updated) return null
    return { ...updated, config: decryptConfig(updated.config) }
}

export async function deleteProvider(id: string): Promise<boolean> {
    const [updated] = await db
        .update(webSearchProviders)
        .set({ isDeleted: true, isCurrent: false, updatedAt: new Date() })
        .where(eq(webSearchProviders.id, id))
        .returning()

    return !!updated
}

export async function setCurrentProvider(
    id: string,
): Promise<{ previous: WebSearchProvider | null }> {
    const previous = await getCurrentProvider()

    await db
        .update(webSearchProviders)
        .set({ isCurrent: false, updatedAt: new Date() })
        .where(eq(webSearchProviders.isCurrent, true))

    await db
        .update(webSearchProviders)
        .set({ isCurrent: true, updatedAt: new Date() })
        .where(and(eq(webSearchProviders.id, id), eq(webSearchProviders.isDeleted, false)))

    return { previous }
}

export { type WebSearchProvider }
