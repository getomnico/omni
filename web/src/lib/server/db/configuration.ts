import { and, eq, sql } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'

export type ConfigurationValue = Record<string, unknown>
export type DoclingQualityPreset = 'fast' | 'balanced' | 'quality'
export type MemoryMode = 'off' | 'chat' | 'full'

export const GLOBAL_CONFIGURATION_KEYS = {
    DOCLING_ENABLED: 'docling_enabled',
    DOCLING_QUALITY_PRESET: 'docling_quality_preset',
    MEMORY_MODE_DEFAULT: 'memory_mode_default',
    MEMORY_LLM_ID: 'memory_llm_id',
} as const

export interface GlobalScopedConfiguration {
    docling_enabled: { enabled: boolean }
    docling_quality_preset: { preset: DoclingQualityPreset }
    memory_mode_default: { value: MemoryMode }
    memory_llm_id: { value: string }
}

export type GlobalConfigurationKey = keyof GlobalScopedConfiguration
const VALID_DOCLING_PRESETS = new Set<DoclingQualityPreset>(['fast', 'balanced', 'quality'])
const VALID_MEMORY_MODES = new Set<MemoryMode>(['off', 'chat', 'full'])

function configurationShapeError(scope: 'global' | 'user', key: string, message: string): Error {
    return new Error(`Invalid ${scope} configuration value for "${key}": ${message}`)
}

function expectRecord(
    scope: 'global' | 'user',
    key: string,
    value: unknown,
): Record<string, unknown> {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
        throw configurationShapeError(scope, key, 'expected an object')
    }
    return value as Record<string, unknown>
}

function expectBoolean(scope: 'global' | 'user', key: string, value: unknown): boolean {
    if (typeof value !== 'boolean') {
        throw configurationShapeError(scope, key, 'expected a boolean')
    }
    return value
}

function expectString(scope: 'global' | 'user', key: string, value: unknown): string {
    if (typeof value !== 'string') {
        throw configurationShapeError(scope, key, 'expected a string')
    }
    return value
}

function expectDoclingPreset(key: string, value: unknown): DoclingQualityPreset {
    const preset = expectString('global', key, value)
    if (!VALID_DOCLING_PRESETS.has(preset as DoclingQualityPreset)) {
        throw configurationShapeError('global', key, 'expected fast, balanced, or quality')
    }
    return preset as DoclingQualityPreset
}

function expectMemoryMode(scope: 'global' | 'user', key: string, value: unknown): MemoryMode {
    const mode = expectString(scope, key, value)
    if (!VALID_MEMORY_MODES.has(mode as MemoryMode)) {
        throw configurationShapeError(scope, key, 'expected off, chat, or full')
    }
    return mode as MemoryMode
}

function parseGlobalConfigurationValue<K extends GlobalConfigurationKey>(
    key: K,
    value: unknown,
): GlobalScopedConfiguration[K] {
    const record = expectRecord('global', key, value)

    switch (key) {
        case GLOBAL_CONFIGURATION_KEYS.DOCLING_ENABLED:
            return {
                enabled: expectBoolean('global', key, record.enabled),
            } as GlobalScopedConfiguration[K]
        case GLOBAL_CONFIGURATION_KEYS.DOCLING_QUALITY_PRESET:
            return {
                preset: expectDoclingPreset(key, record.preset),
            } as GlobalScopedConfiguration[K]
        case GLOBAL_CONFIGURATION_KEYS.MEMORY_MODE_DEFAULT:
            return {
                value: expectMemoryMode('global', key, record.value),
            } as GlobalScopedConfiguration[K]
        case GLOBAL_CONFIGURATION_KEYS.MEMORY_LLM_ID:
            return {
                value: expectString('global', key, record.value),
            } as GlobalScopedConfiguration[K]
    }
}

function asConfigurationValue(value: Record<string, unknown>): ConfigurationValue {
    return value
}

/**
 * Get a global-scope configuration value by key. Returns null if not found.
 */
export async function getGlobal(key: string): Promise<ConfigurationValue | null> {
    const [row] = await db
        .select({ value: configuration.value })
        .from(configuration)
        .where(and(eq(configuration.scope, 'global'), eq(configuration.key, key)))
        .limit(1)
    return (row?.value as ConfigurationValue | undefined) ?? null
}

/**
 * Get a typed global-scope configuration value by key. Returns null if not found.
 */
export async function getTypedGlobal<K extends GlobalConfigurationKey>(
    key: K,
): Promise<GlobalScopedConfiguration[K] | null> {
    const value = await getGlobal(key)
    return value === null ? null : parseGlobalConfigurationValue(key, value)
}

/**
 * Get a per-user configuration value. Returns null if not found.
 */
export async function getUser(userId: string, key: string): Promise<ConfigurationValue | null> {
    const [row] = await db
        .select({ value: configuration.value })
        .from(configuration)
        .where(
            and(
                eq(configuration.scope, 'user'),
                eq(configuration.userId, userId),
                eq(configuration.key, key),
            ),
        )
        .limit(1)
    return (row?.value as ConfigurationValue | undefined) ?? null
}

/**
 * Upsert a global-scope configuration value.
 */
export async function setGlobal(key: string, value: ConfigurationValue): Promise<void> {
    // The unique index is partial (`WHERE scope = 'global'`), which Drizzle's
    // `onConflictDoUpdate` doesn't model directly — fall back to raw SQL.
    const json = JSON.stringify(value)
    await db.execute(sql`
        INSERT INTO configuration (scope, user_id, key, value)
        VALUES ('global', NULL, ${key}, ${json}::jsonb)
        ON CONFLICT (key) WHERE scope = 'global'
        DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()
    `)
}

/**
 * Upsert a typed global-scope configuration value.
 */
export async function setTypedGlobal<K extends GlobalConfigurationKey>(
    key: K,
    value: GlobalScopedConfiguration[K],
): Promise<void> {
    const parsed = parseGlobalConfigurationValue(key, value)
    await setGlobal(key, asConfigurationValue(parsed))
}

/**
 * Upsert a per-user configuration value.
 */
export async function setUser(
    userId: string,
    key: string,
    value: ConfigurationValue,
): Promise<void> {
    const json = JSON.stringify(value)
    await db.execute(sql`
        INSERT INTO configuration (scope, user_id, key, value)
        VALUES ('user', ${userId}, ${key}, ${json}::jsonb)
        ON CONFLICT (user_id, key) WHERE scope = 'user'
        DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()
    `)
}

/**
 * Delete a per-user configuration value, falling back to the global default.
 */
export async function deleteUser(userId: string, key: string): Promise<void> {
    await db
        .delete(configuration)
        .where(
            and(
                eq(configuration.scope, 'user'),
                eq(configuration.userId, userId),
                eq(configuration.key, key),
            ),
        )
}
