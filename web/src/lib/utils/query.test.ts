import { describe, it, expect } from 'vitest'
import { normalizeQuery, normalizedQueryLength, hasMinimumQueryLength } from './query'

// ---------------------------------------------------------------------------
// normalizeQuery
// ---------------------------------------------------------------------------

describe('normalizeQuery', () => {
    it('lowercases the input', () => {
        expect(normalizeQuery('HelloWorld')).toBe('helloworld')
    })

    it('replaces non-alphanumeric characters with spaces', () => {
        expect(normalizeQuery('Budget (Q4-2024).xlsx')).toBe('budget q4 2024 xlsx')
    })

    it('collapses consecutive spaces', () => {
        expect(normalizeQuery('  hello   world  ')).toBe('hello world')
    })

    it('handles empty string', () => {
        expect(normalizeQuery('')).toBe('')
    })

    it('handles only punctuation', () => {
        expect(normalizeQuery('---')).toBe('')
        expect(normalizeQuery('!@#$%^&*()')).toBe('')
    })

    it('preserves Unicode alphabetic characters and marks', () => {
        expect(normalizeQuery('文件名')).toBe('文件名')
        expect(normalizeQuery('हिंदी')).toBe('हिंदी')
    })

    it('preserves digits', () => {
        expect(normalizeQuery('Q4 2024')).toBe('q4 2024')
    })

    it('handles mixed content', () => {
        expect(normalizeQuery('Hello, World! #test123')).toBe('hello world test123')
    })
})

// ---------------------------------------------------------------------------
// normalizedQueryLength
// ---------------------------------------------------------------------------

describe('normalizedQueryLength', () => {
    it('counts ASCII characters', () => {
        expect(normalizedQueryLength('abc')).toBe(3)
        expect(normalizedQueryLength('hello')).toBe(5)
    })

    it('counts Unicode code points rather than UTF-16 units', () => {
        expect(normalizedQueryLength('文')).toBe(1)
        expect(normalizedQueryLength('文件名')).toBe(3)
        expect(normalizedQueryLength('𐐀a')).toBe(2)
    })

    it('strips punctuation before counting', () => {
        expect(normalizedQueryLength('---')).toBe(0)
        // "A-B-C" normalises to "a b c" (3 letters + 2 inter-word spaces = 5)
        expect(normalizedQueryLength('A-B-C')).toBe(5)
    })

    it('collapses whitespace before counting', () => {
        // "  a  b  c  " normalises to "a b c" (3 letters + 2 spaces = 5)
        expect(normalizedQueryLength('  a  b  c  ')).toBe(5)
    })
})

// ---------------------------------------------------------------------------
// hasMinimumQueryLength
// ---------------------------------------------------------------------------

describe('hasMinimumQueryLength', () => {
    it('rejects empty string', () => {
        expect(hasMinimumQueryLength('')).toBe(false)
    })

    it('rejects single character', () => {
        expect(hasMinimumQueryLength('a')).toBe(false)
        expect(hasMinimumQueryLength('文')).toBe(false)
    })

    it('rejects two characters', () => {
        expect(hasMinimumQueryLength('ab')).toBe(false)
        expect(hasMinimumQueryLength('𐐀a')).toBe(false)
    })

    it('accepts three ASCII characters', () => {
        expect(hasMinimumQueryLength('abc')).toBe(true)
    })

    it('accepts three CJK characters', () => {
        expect(hasMinimumQueryLength('文件名')).toBe(true)
    })

    it('rejects punctuation-only strings', () => {
        expect(hasMinimumQueryLength('---')).toBe(false)
        expect(hasMinimumQueryLength('!@!')).toBe(false)
    })

    it('normalizes before checking (punctuation becomes separators)', () => {
        // "A-B-C" normalizes to "a b c" → 5 code points
        expect(hasMinimumQueryLength('A-B-C')).toBe(true)
    })

    it('matches backend behavior for known cases', () => {
        // These match services/searcher/src/typeahead.rs test_minimum_query_length_counts_normalized_unicode_characters
        expect(hasMinimumQueryLength('')).toBe(false)
        expect(hasMinimumQueryLength('a')).toBe(false)
        expect(hasMinimumQueryLength('ab')).toBe(false)
        expect(hasMinimumQueryLength('abc')).toBe(true)
        expect(hasMinimumQueryLength('文')).toBe(false)
        expect(hasMinimumQueryLength('文件名')).toBe(true)
        expect(hasMinimumQueryLength(' A-B-C ')).toBe(true)
    })
})
