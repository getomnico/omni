import type { UserConfiguration, UserMemoryMode } from '$lib/types/configuration'

export type { UserConfiguration, UserMemoryMode } from '$lib/types/configuration'

export const DEFAULT_TIMEZONE = 'UTC'

export const USER_CONFIGURATION_KEYS = {
    MEMORY_MODE: 'memory_mode',
    TIMEZONE: 'timezone',
} as const

export type UserConfigurationKey =
    (typeof USER_CONFIGURATION_KEYS)[keyof typeof USER_CONFIGURATION_KEYS]

const USER_MEMORY_MODES = new Set<UserMemoryMode>(['off', 'chat', 'full'])
const TIMEZONE_ALIASES: Record<string, string> = {
    'Asia/Calcutta': 'Asia/Kolkata',
}

function extractStringValue(raw: unknown, alternateKeys: string[] = []): string | null {
    if (typeof raw === 'string') return raw
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
        const record = raw as Record<string, unknown>
        const candidates = ['value', ...alternateKeys]
        for (const key of candidates) {
            const value = record[key]
            if (typeof value === 'string') return value
        }
    }
    return null
}

export function normalizeTimezone(timezone: string): string | null {
    const candidate = timezone.trim()
    if (!candidate) return null

    try {
        const resolved = new Intl.DateTimeFormat('en-US', {
            timeZone: candidate,
        }).resolvedOptions().timeZone
        return TIMEZONE_ALIASES[resolved] ?? resolved
    } catch {
        return null
    }
}

export function isValidTimezone(timezone: string): boolean {
    return normalizeTimezone(timezone) !== null
}

export function extractUserTimezone(raw: unknown): string | null {
    const value = extractStringValue(raw, ['timezone'])
    if (!value) return null
    return normalizeTimezone(value)
}

export function extractUserMemoryMode(raw: unknown): UserMemoryMode | null {
    const value = extractStringValue(raw, ['mode'])
    if (!value || !USER_MEMORY_MODES.has(value as UserMemoryMode)) return null
    return value as UserMemoryMode
}

export function assertUserMemoryMode(mode: string): asserts mode is UserMemoryMode {
    if (!USER_MEMORY_MODES.has(mode as UserMemoryMode)) {
        throw new Error('Invalid memory mode')
    }
}
