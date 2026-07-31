import { describe, expect, it } from 'vitest'
import { buildDarwinboxConfig, extractApiError } from './darwinbox-config'
import type { DarwinboxManifestExtraSchema, DarwinboxSourceConfig } from './types'

const schema: DarwinboxManifestExtraSchema = {
    sync_capabilities: [
        { name: 'employee_directory', available: true },
        { name: 'holidays', available: false },
    ],
    action_capabilities: [
        { name: 'profile', module: 'self', mode: 'read', endpoints: ['/employee'] },
        { name: 'apply', module: 'self', mode: 'write', endpoints: ['/leave'] },
    ],
}
const base = {
    baseUrl: 'https://example.com/',
    readOnly: false,
    selectedSyncModules: ['employee_directory', 'holidays'],
    selectedActions: ['profile'],
    participantEmails: ['a@example.com'],
    employeeScope: { mode: 'all' as const },
    employeeFields: ['name' as const, 'company_email' as const],
    writeAcknowledged: false,
}

describe('Darwinbox config builder', () => {
    it('includes only selected available capabilities', () => {
        const config = buildDarwinboxConfig(base, schema)
        expect(config.sync_modules).toMatchObject({ employee_directory: true })
        expect(config.sync_modules?.holidays).not.toBe(true)
        expect(config.authorization?.allowed_actions).toEqual(['profile'])
    })
    it('requires company email for organization-visible People identity', () => {
        expect(() => buildDarwinboxConfig({ ...base, employeeFields: ['name'] }, schema)).toThrow(
            /company email/,
        )
    })
    it('requires acknowledgement for selected writes', () => {
        expect(() => buildDarwinboxConfig({ ...base, selectedActions: ['apply'] }, schema)).toThrow(
            /acknowledgement/,
        )
    })
    it('does not emit an empty module key for module-less actions', () => {
        const modulelessSchema: DarwinboxManifestExtraSchema = {
            ...schema,
            action_capabilities: [
                { name: 'find_employee', module: '', mode: 'read', endpoints: ['/employee'] },
            ],
        }
        const config = buildDarwinboxConfig(
            { ...base, selectedActions: ['find_employee'] },
            modulelessSchema,
        )
        expect(Object.hasOwn(config.action_modules ?? {}, '')).toBe(false)
    })
    it('removes writes in read-only mode', () => {
        const config = buildDarwinboxConfig(
            { ...base, readOnly: true, selectedActions: ['profile', 'apply'] },
            schema,
        )
        expect(config.authorization?.allowed_actions).toEqual(['profile'])
    })
    it('excludes unavailable modules and unknown actions', () => {
        const config = buildDarwinboxConfig({ ...base, selectedActions: ['missing'] }, schema)
        expect(config.authorization?.actions_enabled).toBe(false)
        expect(config.sync_modules?.holidays).not.toBe(true)
    })
    it('preserves unrelated existing edit configuration', () => {
        const existing: DarwinboxSourceConfig = {
            base_url: 'old',
            default_timezone: 'UTC',
            authorization: { allowed_report_ids: ['r1'], max_requests_per_minute: 9 },
            sync_modules: { employee_directory: true },
        }
        const config = buildDarwinboxConfig(base, schema, existing)
        expect(config.default_timezone).toBe('UTC')
        expect(config.authorization?.allowed_report_ids).toEqual(['r1'])
        expect(config.authorization?.max_requests_per_minute).toBe(9)
    })
    it('rejects an empty include scope', () => {
        expect(() =>
            buildDarwinboxConfig(
                {
                    ...base,
                    employeeScope: {
                        mode: 'include',
                        employee_ids: [],
                        employee_emails: [],
                        departments: [],
                    },
                },
                schema,
            ),
        ).toThrow(/scope/)
    })
    it('validates base URL security', () => {
        expect(() =>
            buildDarwinboxConfig({ ...base, baseUrl: 'http://example.com' }, schema),
        ).toThrow(/HTTPS/)
        expect(() =>
            buildDarwinboxConfig({ ...base, baseUrl: 'https://user:pass@example.com' }, schema),
        ).toThrow(/credentials/)
        expect(
            buildDarwinboxConfig({ ...base, baseUrl: 'http://localhost:8080/' }, schema).base_url,
        ).toBe('http://localhost:8080')
    })
    it('validates and deduplicates participant emails', () => {
        const config = buildDarwinboxConfig(
            { ...base, participantEmails: [' A@example.com ', 'a@example.com'] },
            schema,
        )
        expect(config.authorization?.participant_emails).toEqual(['a@example.com'])
        expect(() =>
            buildDarwinboxConfig({ ...base, participantEmails: ['invalid'] }, schema),
        ).toThrow(/valid email/)
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
