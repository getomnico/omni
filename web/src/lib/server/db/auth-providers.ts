import { eq } from 'drizzle-orm'
import { db } from './index'
import { authProviders } from './schema'
import type { AuthProvider } from './schema'

export interface GoogleAuthConfig {
    clientId: string
    clientSecret: string
}

export async function getAuthProvider(provider: string): Promise<AuthProvider | null> {
    const [row] = await db
        .select()
        .from(authProviders)
        .where(eq(authProviders.provider, provider))
        .limit(1)
    return row || null
}

export async function getGoogleAuthConfig(): Promise<{
    enabled: boolean
    clientId: string
    clientSecret: string
} | null> {
    const row = await getAuthProvider('google')
    if (!row) return null

    const config = row.config as GoogleAuthConfig
    return {
        enabled: row.enabled,
        clientId: config.clientId || '',
        clientSecret: config.clientSecret || '',
    }
}

export async function updateAuthProvider(
    provider: string,
    enabled: boolean,
    config: Record<string, unknown>,
    updatedBy: string,
): Promise<AuthProvider> {
    const [row] = await db
        .insert(authProviders)
        .values({
            provider,
            enabled,
            config,
            updatedBy,
            updatedAt: new Date(),
        })
        .onConflictDoUpdate({
            target: authProviders.provider,
            set: {
                enabled,
                config,
                updatedBy,
                updatedAt: new Date(),
            },
        })
        .returning()

    return row
}
