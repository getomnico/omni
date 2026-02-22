import { eq } from 'drizzle-orm'
import { db } from './index'
import { connectorConfigs } from './schema'
import type { ConnectorConfig } from './schema'

export interface ConnectorConfigPublic {
    provider: string
    config: Record<string, unknown>
    updatedAt: Date
}

function stripSecrets(config: Record<string, unknown>): Record<string, unknown> {
    const stripped = { ...config }
    if ('oauth_client_secret' in stripped) {
        stripped.oauth_client_secret = '••••••••'
    }
    return stripped
}

export async function getConnectorConfig(provider: string): Promise<ConnectorConfig | null> {
    const [row] = await db
        .select()
        .from(connectorConfigs)
        .where(eq(connectorConfigs.provider, provider))
        .limit(1)
    return row || null
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
        config: stripSecrets(row.config as Record<string, unknown>),
        updatedAt: row.updatedAt,
    }))
}

export async function upsertConnectorConfig(
    provider: string,
    config: Record<string, unknown>,
    updatedBy: string,
): Promise<ConnectorConfig> {
    const [row] = await db
        .insert(connectorConfigs)
        .values({
            provider,
            config,
            updatedBy,
            updatedAt: new Date(),
        })
        .onConflictDoUpdate({
            target: connectorConfigs.provider,
            set: {
                config,
                updatedBy,
                updatedAt: new Date(),
            },
        })
        .returning()

    return row
}
