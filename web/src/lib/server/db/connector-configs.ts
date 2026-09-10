import { eq } from 'drizzle-orm'
import { db } from './index'
import { connectorConfigs } from './schema'
import type { ConnectorConfig } from './schema'
import { decryptConfig, encryptConfig } from '../crypto/encryption'

const SECRET_KEYS = [
    'oauth_client_secret',
    'oauth_registration_initial_access_token',
    'oauth_registration_access_token',
] as const

export interface ConnectorConfigPublic {
    provider: string
    config: Record<string, unknown>
    updatedAt: Date
}

function decryptOAuthSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const decrypted = { ...config }
    for (const key of SECRET_KEYS) {
        const value = decrypted[key]
        if (value && typeof value === 'object') {
            try {
                const plaintext = decryptConfig(value)
                if (typeof plaintext.value === 'string') decrypted[key] = plaintext.value
            } catch {
                // Preserve malformed/legacy values so callers can report a
                // configuration error rather than silently using another secret.
            }
        }
    }
    return decrypted
}

function encryptOAuthSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const encrypted = { ...config }
    for (const key of SECRET_KEYS) {
        const value = encrypted[key]
        if (typeof value === 'string' && value && value !== '••••••••') {
            encrypted[key] = encryptConfig({ value })
        }
    }
    return encrypted
}

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const stripped = { ...config }
    for (const key of [
        'oauth_client_secret',
        'oauth_registration_initial_access_token',
        'oauth_registration_access_token',
    ]) {
        if (key in stripped) stripped[key] = '••••••••'
    }
    return stripped
}

export async function getConnectorConfig(provider: string): Promise<ConnectorConfig | null> {
    const [row] = await db
        .select()
        .from(connectorConfigs)
        .where(eq(connectorConfigs.provider, provider))
        .limit(1)
    if (!row) return null
    return {
        ...row,
        config: decryptOAuthSecrets(row.config as Record<string, unknown>),
    }
}

export async function getConnectorConfigPublic(
    provider: string,
): Promise<ConnectorConfigPublic | null> {
    const row = await getConnectorConfig(provider)
    if (!row) return null

    return {
        provider: row.provider,
        config: stripSecrets(row.config as Record<string, unknown>),
        updatedAt: row.updatedAt,
    }
}

export async function getAllConnectorConfigsPublic(): Promise<ConnectorConfigPublic[]> {
    const rows = await db.select().from(connectorConfigs)
    return rows.map((row) => ({
        provider: row.provider,
        config: stripSecrets(
            decryptOAuthSecrets(row.config as Record<string, unknown>),
        ),
        updatedAt: row.updatedAt,
    }))
}

export async function deleteConnectorConfig(provider: string): Promise<void> {
    await db.delete(connectorConfigs).where(eq(connectorConfigs.provider, provider))
}

export async function upsertConnectorConfig(
    provider: string,
    config: Record<string, unknown>,
    updatedBy: string | null,
): Promise<ConnectorConfig> {
    const [row] = await db
        .insert(connectorConfigs)
        .values({
            provider,
            config: encryptOAuthSecrets(config),
            updatedBy,
            updatedAt: new Date(),
        })
        .onConflictDoUpdate({
            target: connectorConfigs.provider,
            set: {
                config: encryptOAuthSecrets(config),
                updatedBy,
                updatedAt: new Date(),
            },
        })
        .returning()

    return row
}
