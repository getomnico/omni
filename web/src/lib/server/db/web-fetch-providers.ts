import { eq, and } from 'drizzle-orm'
import { db } from './index'
import { webFetchProviders } from './schema'
import type { WebFetchProvider } from './schema'
import { ulid } from 'ulid'
import { encryptConfig, decryptConfig } from '$lib/server/crypto/encryption'

export {
    WEB_FETCH_PROVIDER_TYPES,
    WEB_FETCH_PROVIDER_LABELS,
    type WebFetchProviderType,
} from '$lib/types'
import type { WebFetchProviderType } from '$lib/types'

export interface WebFetchProviderConfig {
    apiKey?: string | null
    baseUrl?: string | null
}

export interface CreateWebFetchProviderInput {
    name: string
    providerType: WebFetchProviderType
    config: WebFetchProviderConfig
}

export interface UpdateWebFetchProviderInput {
    name?: string
    config?: WebFetchProviderConfig
}

export async function listActiveProviders(): Promise<WebFetchProvider[]> {
    const rows = await db
        .select()
        .from(webFetchProviders)
        .where(eq(webFetchProviders.isDeleted, false))
        .orderBy(webFetchProviders.createdAt)
    return rows.map((row) => ({ ...row, config: decryptConfig(row.config) }))
}

export async function getProvider(id: string): Promise<WebFetchProvider | null> {
    const [provider] = await db
        .select()
        .from(webFetchProviders)
        .where(eq(webFetchProviders.id, id))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function getCurrentProvider(): Promise<WebFetchProvider | null> {
    const [provider] = await db
        .select()
        .from(webFetchProviders)
        .where(and(eq(webFetchProviders.isCurrent, true), eq(webFetchProviders.isDeleted, false)))
        .limit(1)
    if (!provider) return null
    return { ...provider, config: decryptConfig(provider.config) }
}

export async function createProvider(input: CreateWebFetchProviderInput): Promise<WebFetchProvider> {
    const existing = await getCurrentProvider()
    const [provider] = await db
        .insert(webFetchProviders)
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
    input: UpdateWebFetchProviderInput,
): Promise<WebFetchProvider | null> {
    const values: Record<string, unknown> = { updatedAt: new Date() }
    if (input.name !== undefined) values.name = input.name
    if (input.config !== undefined) {
        values.config = encryptConfig(input.config as Record<string, unknown>)
    }

    const [updated] = await db
        .update(webFetchProviders)
        .set(values)
        .where(eq(webFetchProviders.id, id))
        .returning()

    if (!updated) return null
    return { ...updated, config: decryptConfig(updated.config) }
}

export async function deleteProvider(id: string): Promise<boolean> {
    const [updated] = await db
        .update(webFetchProviders)
        .set({ isDeleted: true, isCurrent: false, updatedAt: new Date() })
        .where(eq(webFetchProviders.id, id))
        .returning()

    return !!updated
}

export async function setCurrentProvider(id: string): Promise<{ previous: WebFetchProvider | null }> {
    const previous = await getCurrentProvider()

    await db
        .update(webFetchProviders)
        .set({ isCurrent: false, updatedAt: new Date() })
        .where(eq(webFetchProviders.isCurrent, true))

    await db
        .update(webFetchProviders)
        .set({ isCurrent: true, updatedAt: new Date() })
        .where(and(eq(webFetchProviders.id, id), eq(webFetchProviders.isDeleted, false)))

    return { previous }
}

export { type WebFetchProvider }
