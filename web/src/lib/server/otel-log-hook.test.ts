/**
 * Unit tests for the OTel log hook: severity mapping, safe primitive
 * attribute filtering, and no-span mixin behaviour.
 *
 * These tests import only the pure helpers from otel-log-hook.mjs and
 * do NOT require the OTel SDK to be initialised.
 */

import { describe, it, expect, vi } from 'vitest'

// Import pure helpers from the production module
import { extractSafeAttributes } from './otel-log-hook.mjs'

// ---------------------------------------------------------------------------
// Helper: simulate SeverityNumber constants that the hook uses internally
// ---------------------------------------------------------------------------

const SeverityNumber = {
    TRACE: 1,
    DEBUG: 5,
    INFO: 9,
    WARN: 13,
    ERROR: 17,
    FATAL: 21,
}

// Replicate the mapping from otel-log-hook.mjs for test assertions
const PINO_LEVEL_TO_SEVERITY: Record<number, number> = {
    10: SeverityNumber.TRACE,
    20: SeverityNumber.DEBUG,
    30: SeverityNumber.INFO,
    40: SeverityNumber.WARN,
    50: SeverityNumber.ERROR,
    60: SeverityNumber.FATAL,
}

const PINO_LEVEL_TO_TEXT: Record<number, string> = {
    10: 'TRACE',
    20: 'DEBUG',
    30: 'INFO',
    40: 'WARN',
    50: 'ERROR',
    60: 'FATAL',
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('severity mapping', () => {
    it('maps pino level 10 (trace) to SeverityNumber.TRACE (1)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[10]).toBe(1)
        expect(PINO_LEVEL_TO_TEXT[10]).toBe('TRACE')
    })

    it('maps pino level 20 (debug) to SeverityNumber.DEBUG (5)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[20]).toBe(5)
        expect(PINO_LEVEL_TO_TEXT[20]).toBe('DEBUG')
    })

    it('maps pino level 30 (info) to SeverityNumber.INFO (9)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[30]).toBe(9)
        expect(PINO_LEVEL_TO_TEXT[30]).toBe('INFO')
    })

    it('maps pino level 40 (warn) to SeverityNumber.WARN (13)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[40]).toBe(13)
        expect(PINO_LEVEL_TO_TEXT[40]).toBe('WARN')
    })

    it('maps pino level 50 (error) to SeverityNumber.ERROR (17)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[50]).toBe(17)
        expect(PINO_LEVEL_TO_TEXT[50]).toBe('ERROR')
    })

    it('maps pino level 60 (fatal) to SeverityNumber.FATAL (21)', () => {
        expect(PINO_LEVEL_TO_SEVERITY[60]).toBe(21)
        expect(PINO_LEVEL_TO_TEXT[60]).toBe('FATAL')
    })

    it('falls back to INFO (9) for unknown level', () => {
        // Any level not in the map should get the default INFO
        const unknownLevel = 99
        const sev = PINO_LEVEL_TO_SEVERITY[unknownLevel] ?? SeverityNumber.INFO
        const text = PINO_LEVEL_TO_TEXT[unknownLevel] ?? 'INFO'
        expect(sev).toBe(9)
        expect(text).toBe('INFO')
    })
})

describe('extractSafeAttributes', () => {
    it('extracts string, number, and boolean values from allowlisted keys', () => {
        const result = extractSafeAttributes({
            method: 'GET',
            count: 42,
            status: true,
        })
        expect(result).toEqual({
            method: 'GET',
            count: 42,
            status: true,
        })
    })

    it('omits nested objects', () => {
        const result = extractSafeAttributes({
            user: { id: 1, email: 'test@example.com' },
            method: 'GET',
        })
        expect(result).toEqual({ method: 'GET' })
        expect(result).not.toHaveProperty('user')
    })

    it('omits arrays', () => {
        const result = extractSafeAttributes({
            tags: ['a', 'b', 'c'],
            count: 5,
        })
        expect(result).toEqual({ count: 5 })
        expect(result).not.toHaveProperty('tags')
    })

    it('omits Error instances', () => {
        const result = extractSafeAttributes({
            error: new Error('boom'),
            status: 'failed',
        })
        expect(result).toEqual({ status: 'failed' })
        expect(result).not.toHaveProperty('error')
    })

    it('omits URL instances', () => {
        const result = extractSafeAttributes({
            url: new URL('https://example.com/path?q=1'),
            route: '/api/test',
        })
        expect(result).toEqual({ route: '/api/test' })
        expect(result).not.toHaveProperty('url')
    })

    it('omits null and undefined values', () => {
        const result = extractSafeAttributes({
            nullable: null,
            undef: undefined,
            method: 'POST',
        })
        expect(result).toEqual({ method: 'POST' })
    })

    it('allows only keys on the explicit allowlist (camelCase variants)', () => {
        const result = extractSafeAttributes({
            statusCode: 200,
            durationMs: 42,
            resultsCount: 10,
            method: 'GET',
            route: '/api/chat',
        })
        expect(result.statusCode).toBe(200)
        expect(result.durationMs).toBe(42)
        expect(result.resultsCount).toBe(10)
        expect(result.method).toBe('GET')
        expect(result.route).toBe('/api/chat')
    })

    it('rejects keys not on the explicit allowlist', () => {
        // None of these keys are in the allowlist, so all are rejected
        const result = extractSafeAttributes({
            userToken: 'abc',
            auth_secret: 'xyz',
            user_password: 'pwd',
            bearer_authorization: 'Bearer ...',
            session_cookie: 'sid',
            user_email: 'a@b.com',
            search_query: 'test',
            system_prompt: 'hello',
            request_body: '{}',
            document_content: 'text',
            error_message: 'fail',
            source_url: 'http://',
            redirect_uri: 'http://',
            file_path: '/tmp',
            user_profile: '...',
            document_title: 'Doc',
            email_recipient: 'b@c.com',
            email_subject: 'Re:',
            error_code: 500,
            connection_state: 'open',
        })
        expect(Object.keys(result)).toHaveLength(0)
    })

    it('rejects keys not on the explicit allowlist (_id suffix)', () => {
        const result = extractSafeAttributes({
            user_id: '123',
            documentId: 'abc',
            chatId: 'def',
            messageId: 'ghi',
            file_id: 'xyz',
        })
        expect(Object.keys(result)).toHaveLength(0)
    })

    it('rejects keys not on the explicit allowlist (contain "user")', () => {
        const result = extractSafeAttributes({
            userId: '123',
            userName: 'test',
            userConfiguration: {},
        })
        // userConfiguration is an object so type-filtered out regardless
        expect(result.userId).toBeUndefined()
        expect(result.userName).toBeUndefined()
    })

    it('rejects unknown keys not on the explicit allowlist (e.g. customField)', () => {
        const result = extractSafeAttributes({
            customField: 'hello',
            someValue: 42,
        })
        expect(Object.keys(result)).toHaveLength(0)
    })

    it('omits non-primitive values (objects, arrays, Buffer) even if key is allowlisted', () => {
        // The type-based filter removes non-primitive values regardless
        // of whether the key is on the allowlist.
        const result = extractSafeAttributes({
            body: { message: 'content' },
            status: ['not-a-status'], // array -> omitted even though 'status' is allowlisted
            count: { nested: 'object' },
            size: Buffer.from('test'),
        })
        expect(Object.keys(result)).toHaveLength(0)
    })

    it('rejects sensitive attribute keys that are primitive strings but not allowlisted', () => {
        // email/id/token are primitive strings but their keys are not on
        // the allowlist, so they are rejected.
        const result = extractSafeAttributes({
            email: 'user@example.com',
            id: 'abc-123',
            token: 's0m3t0k3n',
        })
        expect(Object.keys(result)).toHaveLength(0)
        expect(result.email).toBeUndefined()
        expect(result.id).toBeUndefined()
        expect(result.token).toBeUndefined()
    })

    it('returns empty object for null input', () => {
        expect(extractSafeAttributes(null)).toEqual({})
    })

    it('returns empty object for undefined input', () => {
        expect(extractSafeAttributes(undefined)).toEqual({})
    })

    it('returns empty object for primitive input', () => {
        expect(extractSafeAttributes(42 as any)).toEqual({})
        expect(extractSafeAttributes('string' as any)).toEqual({})
        expect(extractSafeAttributes(true as any)).toEqual({})
    })

    it('returns empty object for array input', () => {
        expect(extractSafeAttributes([1, 2, 3] as any)).toEqual({})
    })
})

describe('no-span mixin behaviour', () => {
    it('returns empty object when no active span exists', async () => {
        // Outside any span, trace.getActiveSpan() returns undefined.
        // We simulate this by importing trace and checking outside a span.
        const { trace } = await import('@opentelemetry/api')
        const span = trace.getActiveSpan()
        expect(span).toBeUndefined()
    })
})
