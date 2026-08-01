import { describe, expect, it } from 'vitest'
import { buildDarwinboxConfig, extractApiError } from './darwinbox-config'
import type { DarwinboxSourceConfig } from './types'

const base = {
    baseUrl: 'https://example.com/',
    readOnly: false,
    participantMode: 'all' as const,
    participantEmails: ['a@example.com'],
}

describe('Darwinbox config builder', () => {
    it('builds a minimal config with normalized base URL', () => {
        const config = buildDarwinboxConfig(base)
        expect(config.base_url).toBe('https://example.com')
        expect(config.read_only).toBe(false)
        expect(config.authorization?.participant_mode).toBe('all')
    })
    it('defaults actions to everyone and clears the allowlist', () => {
        const config = buildDarwinboxConfig(base)
        expect(config.authorization?.participant_mode).toBe('all')
        expect(config.authorization?.participant_emails).toEqual([])
    })
    it('restricts actions when allowlist mode is chosen', () => {
        const config = buildDarwinboxConfig({
            ...base,
            participantMode: 'allowlist',
            participantEmails: [' A@example.com ', 'a@example.com'],
        })
        expect(config.authorization?.participant_mode).toBe('allowlist')
        expect(config.authorization?.participant_emails).toEqual(['a@example.com'])
        expect(() =>
            buildDarwinboxConfig({
                ...base,
                participantMode: 'allowlist',
                participantEmails: ['invalid'],
            }),
        ).toThrow(/valid email/)
        expect(() =>
            buildDarwinboxConfig({ ...base, participantMode: 'allowlist', participantEmails: [] }),
        ).toThrow(/specific people/)
    })
    it('preserves unrelated existing edit configuration', () => {
        const existing: DarwinboxSourceConfig = {
            base_url: 'old',
            default_timezone: 'UTC',
            authorization: { allowed_report_ids: ['r1'], max_batch_size: 3 },
        }
        const config = buildDarwinboxConfig(base, existing)
        expect(config.default_timezone).toBe('UTC')
        expect(config.authorization?.allowed_report_ids).toEqual(['r1'])
        expect(config.authorization?.max_batch_size).toBe(3)
    })
    it('validates base URL security', () => {
        expect(() => buildDarwinboxConfig({ ...base, baseUrl: 'http://example.com' })).toThrow(
            /HTTPS/,
        )
        expect(() =>
            buildDarwinboxConfig({ ...base, baseUrl: 'https://user:pass@example.com' }),
        ).toThrow(/credentials/)
        expect(buildDarwinboxConfig({ ...base, baseUrl: 'http://localhost:8080/' }).base_url).toBe(
            'http://localhost:8080',
        )
    })
    it('extracts connector JSON validation errors', async () => {
        const response = new Response(
            JSON.stringify({ message: 'Invalid config', validation: ['Scope is empty'] }),
            { status: 400, headers: { 'content-type': 'application/json' } },
        )
        await expect(extractApiError(response, 'fallback')).resolves.toBe(
            'Invalid config\nScope is empty',
        )
    })
})
