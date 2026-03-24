import { eq } from 'drizzle-orm'
import { db } from './index'
import { authProviders } from './schema'
import type { AuthProvider } from './schema'
import { encryptConfig, decryptConfig } from '$lib/server/crypto/encryption'

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
    if (!row) return null
    return { ...row, config: decryptConfig(row.config) }
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

export async function getOktaAuthConfig(): Promise<{
    enabled: boolean
    oktaDomain: string
    clientId: string
    clientSecret: string
} | null> {
    const row = await getAuthProvider('okta')
    if (!row) return null
    const config = row.config as { oktaDomain: string; clientId: string; clientSecret: string }
    return {
        enabled: row.enabled,
        oktaDomain: config.oktaDomain || '',
        clientId: config.clientId || '',
        clientSecret: config.clientSecret || '',
    }
}

export async function isPasswordAuthEnabled(): Promise<boolean> {
    const row = await getAuthProvider('password')
    if (!row) return true
    return row.enabled
}

export async function updateAuthProvider(
    provider: string,
    enabled: boolean,
    config: Record<string, unknown>,
    updatedBy: string,
): Promise<AuthProvider> {
    const encryptedConfig = encryptConfig(config)
    const [row] = await db
        .insert(authProviders)
        .values({
            provider,
            enabled,
            config: encryptedConfig,
            updatedBy,
            updatedAt: new Date(),
        })
        .onConflictDoUpdate({
            target: authProviders.provider,
            set: {
                enabled,
                config: encryptedConfig,
                updatedBy,
                updatedAt: new Date(),
            },
        })
        .returning()

    return { ...row, config: decryptConfig(row.config) }
}
