import { describe, expect, it } from 'vitest'
import {
    DEFAULT_TIMEZONE,
    extractUserMemoryMode,
    extractUserTimezone,
    normalizeTimezone,
    USER_CONFIGURATION_KEYS,
} from './userConfigurationSchema'

describe('user configuration schema helpers', () => {
    it('defines typed known keys and timezone default', () => {
        expect(DEFAULT_TIMEZONE).toBe('UTC')
        expect(USER_CONFIGURATION_KEYS.TIMEZONE).toBe('timezone')
        expect(USER_CONFIGURATION_KEYS.MEMORY_MODE).toBe('memory_mode')
    })

    it('normalizes and extracts valid timezone values', () => {
        expect(normalizeTimezone('UTC')).toBe('UTC')
        expect(extractUserTimezone({ value: 'America/New_York' })).toBe('America/New_York')
        expect(extractUserTimezone({ timezone: 'Europe/Berlin' })).toBe('Europe/Berlin')
        expect(normalizeTimezone('Asia/Calcutta')).toBe('Asia/Kolkata')
    })

    it('rejects invalid timezone values', () => {
        expect(normalizeTimezone('Not/AZone')).toBeNull()
        expect(extractUserTimezone({ value: 'Not/AZone' })).toBeNull()
        expect(extractUserTimezone({ value: '' })).toBeNull()
    })

    it('extracts only known memory modes', () => {
        expect(extractUserMemoryMode({ value: 'chat' })).toBe('chat')
        expect(extractUserMemoryMode({ mode: 'full' })).toBe('full')
        expect(extractUserMemoryMode({ value: 'invalid' })).toBeNull()
    })
})
