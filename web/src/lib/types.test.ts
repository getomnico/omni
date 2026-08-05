import { describe, expect, it } from 'vitest'
import { IntegrationType, supportsDataSync } from './types'

describe('supportsDataSync', () => {
    it('allows native connector sources and legacy missing integration types', () => {
        expect(supportsDataSync(IntegrationType.CONNECTOR)).toBe(true)
        expect(supportsDataSync(undefined)).toBe(true)
        expect(supportsDataSync(null)).toBe(true)
    })

    it('rejects remote MCP sources', () => {
        expect(supportsDataSync(IntegrationType.REMOTE_MCP)).toBe(false)
    })
})
