/**
 * Source-level sanitization regression tests for web service logs.
 *
 * These are static-analysis tests that scan the actual source files for
 * forbidden patterns in console.log / console.error / logger calls.
 * No runtime dependencies required.
 */

import { describe, it, expect } from 'vitest'
import { readFileSync } from 'fs'
import { resolve, dirname } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const WEB_ROOT = resolve(__dirname, '../../..')

function readLines(relPath: string): string[] {
    const fullPath = resolve(WEB_ROOT, relPath)
    return readFileSync(fullPath, 'utf-8').split('\n')
}

function isLogLine(line: string): boolean {
    const s = line.trim()
    return (
        s.startsWith('console.log(') ||
        s.startsWith('console.error(') ||
        s.startsWith('console.warn(') ||
        s.startsWith('logger.info(') ||
        s.startsWith('logger.error(') ||
        s.startsWith('logger.warn(')
    )
}

describe('instrumentation.mjs log sanitization', () => {
    const lines = readLines('src/instrumentation.mjs')

    it('must not print the OTLP endpoint URL', () => {
        const violating = lines.filter((l) => isLogLine(l) && l.includes('otlpEndpoint'))
        expect(violating).toHaveLength(0)
    })

    it('must not print the shutdown error object or its value', () => {
        const violating = lines.filter((l) => isLogLine(l) && l.includes(', error'))
        expect(violating).toHaveLength(0)
    })

    it('must only print fixed enabled/disabled/shutdown outcome messages', () => {
        const logLines = lines.filter(isLogLine)
        for (const line of logLines) {
            // No dynamic values from variables — only static strings
            expect(line).not.toMatch(/`/)
            expect(line).not.toMatch(/\+.*error/)
        }
    })
})

describe('unlink +server.ts log sanitization', () => {
    const lines = readLines('src/routes/(public)/auth/google/unlink/+server.ts')

    it('must not print the user ID', () => {
        const violating = lines.filter(
            (l) => isLogLine(l) && (l.includes('userSession') || l.includes('user.id')),
        )
        expect(violating).toHaveLength(0)
    })

    it('must not print the error object value', () => {
        const violating = lines.filter((l) => isLogLine(l) && l.includes(', error'))
        expect(violating).toHaveLength(0)
    })
})
