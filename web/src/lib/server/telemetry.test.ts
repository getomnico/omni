/**
 * Unit tests for HTTP RED metric helpers.
 *
 * Imports the actual pure helpers from telemetry.ts to verify conversion
 * and attribute construction.  Does NOT require the SDK bootstrap
 * (instrumentation.mjs) to be loaded.
 */

import { describe, it, expect, vi } from 'vitest'

// Import pure helpers from the production module
import { millisecondsToSeconds, buildRedAttributes } from './telemetry.js'
import { parseMetricExportInterval } from './otel-utils.mjs'

describe('millisecondsToSeconds', () => {
    it('should convert 50ms to 0.05 seconds', () => {
        const secs = millisecondsToSeconds(50)
        expect(secs).toBe(0.05)
        // If the bug existed, secs would be 50 (not dividing by 1000)
        expect(secs).toBeLessThan(1)
    })

    it('should convert 1000ms to 1 second', () => {
        expect(millisecondsToSeconds(1000)).toBe(1)
    })

    it('should handle zero', () => {
        expect(millisecondsToSeconds(0)).toBe(0)
    })

    it('should handle fractional ms', () => {
        expect(millisecondsToSeconds(0.5)).toBe(0.0005)
    })
})

describe('buildRedAttributes', () => {
    it('should use bounded attributes only', () => {
        const attrs = buildRedAttributes('GET', '/ok', 200)
        expect(attrs).toHaveProperty('http.request.method', 'GET')
        expect(attrs).toHaveProperty('http.route', '/ok')
        expect(attrs).toHaveProperty('http.response.status_code', 200)
        expect(Object.keys(attrs)).toHaveLength(3)
    })

    it('should use route template, not raw path', () => {
        const attrs = buildRedAttributes('GET', '/users/:id/details', 200)
        expect(attrs['http.route']).toBe('/users/:id/details')
        expect(attrs['http.route']).not.toContain('user-123')
        expect(attrs['http.route']).not.toContain('?')
    })

    it('should record status as numeric code', () => {
        const attrs = buildRedAttributes('GET', '/error', 500)
        expect(attrs['http.response.status_code']).toBe(500)
    })
})

describe('parseMetricExportInterval', () => {
    it('should return 60000 when undefined', () => {
        expect(parseMetricExportInterval(undefined)).toBe(60000)
    })

    it('should parse valid positive integer', () => {
        expect(parseMetricExportInterval('30000')).toBe(30000)
    })

    it('should fall back for empty string', () => {
        expect(parseMetricExportInterval('')).toBe(60000)
    })

    it('should fall back for NaN', () => {
        expect(parseMetricExportInterval('not-a-number')).toBe(60000)
    })

    it('should fall back for zero', () => {
        expect(parseMetricExportInterval('0')).toBe(60000)
    })

    it('should fall back for negative', () => {
        expect(parseMetricExportInterval('-1')).toBe(60000)
    })
})
