import { describe, expect, it } from 'vitest'
import { parseApiKeyScope } from './apiKeyScope'

describe('parseApiKeyScope', () => {
    it('accepts only public and user scopes', () => {
        expect(parseApiKeyScope('public')).toBe('public')
        expect(parseApiKeyScope('user')).toBe('user')
        expect(parseApiKeyScope('admin')).toBeNull()
        expect(parseApiKeyScope(undefined)).toBeNull()
    })
})
