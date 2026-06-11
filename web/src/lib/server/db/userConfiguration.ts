import { and, eq } from 'drizzle-orm'
import { db } from './index'
import { configuration } from './schema'
import {
    deleteUser as deleteUserConfigurationValue,
    getUser as getUserConfigurationValue,
    setUser as setUserConfigurationValue,
    type ConfigurationValue,
} from './configuration'
import {
    assertUserMemoryMode,
    extractUserMemoryMode,
    extractUserTimezone,
    normalizeTimezone,
    USER_CONFIGURATION_KEYS,
    type UserConfiguration,
    type UserMemoryMode,
} from './userConfigurationSchema'

export {
    DEFAULT_TIMEZONE,
    extractUserMemoryMode,
    extractUserTimezone,
    isValidTimezone,
    normalizeTimezone,
    USER_CONFIGURATION_KEYS,
    type UserConfiguration,
    type UserConfigurationKey,
    type UserMemoryMode,
} from './userConfigurationSchema'

export async function getUserConfiguration(userId: string): Promise<UserConfiguration> {
    const rows = await db
        .select({ key: configuration.key, value: configuration.value })
        .from(configuration)
        .where(and(eq(configuration.scope, 'user'), eq(configuration.userId, userId)))

    const values = new Map<string, unknown>()
    for (const row of rows) values.set(row.key, row.value)

    return {
        memoryMode: extractUserMemoryMode(values.get(USER_CONFIGURATION_KEYS.MEMORY_MODE)),
        timezone: extractUserTimezone(values.get(USER_CONFIGURATION_KEYS.TIMEZONE)),
    }
}

export async function getUserTimezone(userId: string): Promise<string | null> {
    const value = await getUserConfigurationValue(userId, USER_CONFIGURATION_KEYS.TIMEZONE)
    return extractUserTimezone(value)
}

export async function setUserTimezone(userId: string, timezone: string): Promise<string> {
    const normalized = normalizeTimezone(timezone)
    if (!normalized) {
        throw new Error('Invalid timezone')
    }

    await setUserConfigurationValue(userId, USER_CONFIGURATION_KEYS.TIMEZONE, {
        value: normalized,
    })
    return normalized
}

export async function getUserMemoryMode(userId: string): Promise<UserMemoryMode | null> {
    const value = await getUserConfigurationValue(userId, USER_CONFIGURATION_KEYS.MEMORY_MODE)
    return extractUserMemoryMode(value)
}

export async function setUserMemoryMode(userId: string, mode: UserMemoryMode): Promise<void> {
    assertUserMemoryMode(mode)
    await setUserConfigurationValue(userId, USER_CONFIGURATION_KEYS.MEMORY_MODE, { value: mode })
}

export async function deleteUserMemoryMode(userId: string): Promise<void> {
    await deleteUserConfigurationValue(userId, USER_CONFIGURATION_KEYS.MEMORY_MODE)
}

export type { ConfigurationValue }
