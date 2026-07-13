import { describe, it, expect } from 'vitest'
import type { TextCitationParam } from '@anthropic-ai/sdk/resources'
import { normalizeCitation, citationIdFromCitation, sourceIdentityFromCitation } from './citations'

// ---------------------------------------------------------------------------
// citationIdFromCitation
// ---------------------------------------------------------------------------

describe('citationIdFromCitation', () => {
    it('includes title + cited_text for search_result_location so identical ranges with different content resolve separately', () => {
        const a = {
            type: 'search_result_location' as const,
            source: 'imap://a',
            title: 'Title A',
            cited_text: 'cited A',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const b = {
            type: 'search_result_location' as const,
            source: 'imap://a',
            title: 'Title B',
            cited_text: 'cited B',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        // Same source+index+range BUT different title/cited_text → different id
        expect(citationIdFromCitation(a)).not.toBe(citationIdFromCitation(b))
    })

    it('includes title + cited_text for web_search_result_location', () => {
        const a = {
            type: 'web_search_result_location' as const,
            url: 'https://example.com',
            title: 'T1',
            cited_text: 'c1',
            encrypted_index: 'abc',
        }
        const b = {
            type: 'web_search_result_location' as const,
            url: 'https://example.com',
            title: 'T2',
            cited_text: 'c2',
            encrypted_index: 'abc',
        }
        expect(citationIdFromCitation(a)).not.toBe(citationIdFromCitation(b))
    })

    it('includes cited_text for char_location', () => {
        const a = {
            type: 'char_location' as const,
            document_index: 0,
            document_title: 'Doc',
            start_char_index: 0,
            end_char_index: 100,
            cited_text: 'first excerpt',
        }
        const b = {
            type: 'char_location' as const,
            document_index: 0,
            document_title: 'Doc',
            start_char_index: 0,
            end_char_index: 100,
            cited_text: 'second excerpt',
        }
        expect(citationIdFromCitation(a)).not.toBe(citationIdFromCitation(b))
    })

    it('includes cited_text for page_location', () => {
        const a = {
            type: 'page_location' as const,
            document_index: 1,
            document_title: 'Paper',
            start_page_number: 3,
            end_page_number: 3,
            cited_text: 'page text A',
        }
        const b = {
            type: 'page_location' as const,
            document_index: 1,
            document_title: 'Paper',
            start_page_number: 3,
            end_page_number: 3,
            cited_text: 'page text B',
        }
        expect(citationIdFromCitation(a)).not.toBe(citationIdFromCitation(b))
    })

    it('includes cited_text for content_block_location', () => {
        const a = {
            type: 'content_block_location' as const,
            document_index: 2,
            document_title: 'Doc',
            start_block_index: 0,
            end_block_index: 1,
            cited_text: 'block A',
        }
        const b = {
            type: 'content_block_location' as const,
            document_index: 2,
            document_title: 'Doc',
            start_block_index: 0,
            end_block_index: 1,
            cited_text: 'block B',
        }
        expect(citationIdFromCitation(a)).not.toBe(citationIdFromCitation(b))
    })
})

// ---------------------------------------------------------------------------
// sourceIdentityFromCitation (namespaced JSON tuples)
// ---------------------------------------------------------------------------

describe('sourceIdentityFromCitation', () => {
    it('uses namespaced ["source", value] tuples for search results', () => {
        const a = {
            type: 'search_result_location' as const,
            source: 'https://same.com',
            title: null,
            cited_text: 'c',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const b = {
            type: 'search_result_location' as const,
            source: 'https://same.com',
            title: null,
            cited_text: 'd',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 1,
        }
        expect(sourceIdentityFromCitation(a)).toBe(sourceIdentityFromCitation(b))
        expect(sourceIdentityFromCitation(a)).toBe(JSON.stringify(['source', 'https://same.com']))
    })

    it('uses namespaced ["source", value] tuples for web search, cross-type dedup with same URL', () => {
        const a = {
            type: 'web_search_result_location' as const,
            url: 'https://same.com',
            title: 'T',
            cited_text: 'c',
            encrypted_index: 'e1',
        }
        const b = {
            type: 'web_search_result_location' as const,
            url: 'https://same.com',
            title: 'T2',
            cited_text: 'c2',
            encrypted_index: 'e2',
        }
        // Same url → same source id; web and search use same ['source', url] domain
        expect(sourceIdentityFromCitation(a)).toBe(sourceIdentityFromCitation(b))
        expect(sourceIdentityFromCitation(a)).toBe(JSON.stringify(['source', 'https://same.com']))
        // Different url → different source id
        const c = {
            type: 'web_search_result_location' as const,
            url: 'https://other.com',
            title: 'T',
            cited_text: 'c',
            encrypted_index: 'e',
        }
        expect(sourceIdentityFromCitation(a)).not.toBe(sourceIdentityFromCitation(c))
    })

    it('prevents document index from colliding with source domain', () => {
        const a = {
            type: 'char_location' as const,
            document_index: 5,
            document_title: 'My Doc',
            start_char_index: 0,
            end_char_index: 10,
            cited_text: 'c',
        }
        const b = {
            type: 'content_block_location' as const,
            document_index: 5,
            document_title: null,
            start_block_index: 0,
            end_block_index: 1,
            cited_text: 'd',
        }
        const c = {
            type: 'page_location' as const,
            document_index: 5,
            document_title: 'Different',
            start_page_number: 1,
            end_page_number: 1,
            cited_text: 'e',
        }
        // Same document_index → same source
        expect(sourceIdentityFromCitation(a)).toBe(sourceIdentityFromCitation(b))
        expect(sourceIdentityFromCitation(a)).toBe(sourceIdentityFromCitation(c))
        // Namespace prevents collision with ['source', '5']
        expect(sourceIdentityFromCitation(a)).toBe(JSON.stringify(['document', 5]))
        expect(sourceIdentityFromCitation(a)).not.toBe(JSON.stringify(['source', '5']))
    })

    it('separates different document_index values', () => {
        const a = {
            type: 'char_location' as const,
            document_index: 1,
            document_title: 'Doc',
            start_char_index: 0,
            end_char_index: 10,
            cited_text: 'c',
        }
        const b = {
            type: 'page_location' as const,
            document_index: 2,
            document_title: 'Doc',
            start_page_number: 1,
            end_page_number: 1,
            cited_text: 'd',
        }
        expect(sourceIdentityFromCitation(a)).not.toBe(sourceIdentityFromCitation(b))
    })
})

// ---------------------------------------------------------------------------
// normalizeCitation – sourceName and full field coverage
// ---------------------------------------------------------------------------

describe('normalizeCitation', () => {
    it('search_result_location with http source → navigable, sourceName=Web', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'https://example.com/doc',
            title: 'Example Doc',
            cited_text: 'some text',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Example Doc')
        expect(n.href).toBe('https://example.com/doc')
        expect(n.isImap).toBe(false)
        expect(n.locationLabel).toBeNull()
        expect(n.iconHint).toBe('https://example.com/doc')
        expect(n.sourceName).toBe('Web')
    })

    it('search_result_location with imap → not navigable, isImap=true, sourceName=IMAP', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'imap:account / Inbox / 2026-01-01 / subject',
            title: 'Email',
            cited_text: 'text',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Email')
        expect(n.href).toBeNull()
        expect(n.isImap).toBe(true)
        expect(n.sourceName).toBe('IMAP')
    })

    it('search_result_location with google-drive #meta= → navigable, sourceName=Google Drive', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'https://docs.google.com/doc#meta=google_drive,document',
            title: 'GDoc',
            cited_text: 't',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const n = normalizeCitation(c)
        expect(n.href).toBe(c.source)
        expect(n.sourceName).toBe('Google Drive')
    })

    it('search_result_location with slack URL → sourceName=Slack', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'https://slack.com/archives/C123',
            title: 'Slack Msg',
            cited_text: 't',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        expect(normalizeCitation(c).sourceName).toBe('Slack')
    })

    it('search_result_location with synthetic source → Files, not navigable', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'synthetic://x',
            title: 'Synth',
            cited_text: 't',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Synth')
        expect(n.href).toBeNull()
        expect(n.sourceName).toBe('Files')
    })

    it('search_result_location with null title → falls back to source', () => {
        const c = {
            type: 'search_result_location' as const,
            source: 'synthetic://x',
            title: null,
            cited_text: 't',
            start_block_index: 0,
            end_block_index: 0,
            search_result_index: 0,
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('synthetic://x')
        expect(n.sourceName).toBe('Files')
    })

    it('web_search_result_location → navigable, sourceName=Web', () => {
        const c = {
            type: 'web_search_result_location' as const,
            url: 'https://web.example.com/r',
            title: 'Web Result',
            cited_text: 'snippet',
            encrypted_index: 'abc',
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Web Result')
        expect(n.href).toBe('https://web.example.com/r')
        expect(n.isImap).toBe(false)
        expect(n.locationLabel).toBeNull()
        expect(n.sourceName).toBe('Web')
        expect(n.iconHint).toBe('https://web.example.com/r')
    })

    it('web_search_result_location with known connector URL → inferred sourceName', () => {
        const c = {
            type: 'web_search_result_location' as const,
            url: 'https://github.com/org/repo',
            title: 'GH',
            cited_text: 't',
            encrypted_index: 'e',
        }
        expect(normalizeCitation(c).sourceName).toBe('GitHub')
    })

    it('char_location → Document excerpt, Files sourceName', () => {
        const c = {
            type: 'char_location' as const,
            document_index: 0,
            document_title: 'Ref Doc',
            start_char_index: 0,
            end_char_index: 50,
            cited_text: 'excerpt',
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Ref Doc')
        expect(n.href).toBeNull()
        expect(n.isImap).toBe(false)
        expect(n.locationLabel).toBe('Document excerpt')
        expect(n.sourceName).toBe('Files')
    })

    it('char_location with null title → Document N (1-indexed)', () => {
        const c = {
            type: 'char_location' as const,
            document_index: 0,
            document_title: null,
            start_char_index: 0,
            end_char_index: 10,
            cited_text: 't',
        }
        expect(normalizeCitation(c).title).toBe('Document 1')
    })

    it('page_location single page → Page 7, Files', () => {
        const c = {
            type: 'page_location' as const,
            document_index: 3,
            document_title: 'Paper',
            start_page_number: 7,
            end_page_number: 7,
            cited_text: 'text',
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Paper')
        expect(n.href).toBeNull()
        expect(n.locationLabel).toBe('Page 7')
        expect(n.sourceName).toBe('Files')
    })

    it('page_location range → Pages 3–5', () => {
        const c = {
            type: 'page_location' as const,
            document_index: 3,
            document_title: 'Paper',
            start_page_number: 3,
            end_page_number: 5,
            cited_text: 'text',
        }
        expect(normalizeCitation(c).locationLabel).toBe('Pages 3–5')
    })

    it('content_block_location → Document excerpt, Files', () => {
        const c = {
            type: 'content_block_location' as const,
            document_index: 1,
            document_title: 'Block Doc',
            start_block_index: 0,
            end_block_index: 2,
            cited_text: 'block excerpt',
        }
        const n = normalizeCitation(c)
        expect(n.title).toBe('Block Doc')
        expect(n.href).toBeNull()
        expect(n.isImap).toBe(false)
        expect(n.locationLabel).toBe('Document excerpt')
        expect(n.sourceName).toBe('Files')
    })

    it('content_block_location with null title → Document N', () => {
        const c = {
            type: 'content_block_location' as const,
            document_index: 4,
            document_title: null,
            start_block_index: 0,
            end_block_index: 1,
            cited_text: 't',
        }
        expect(normalizeCitation(c).title).toBe('Document 5')
    })

    it('exposes all NormalizedCitation fields for every type', () => {
        const types: Array<TextCitationParam['type']> = [
            'search_result_location',
            'web_search_result_location',
            'char_location',
            'page_location',
            'content_block_location',
        ]
        for (const type of types) {
            const raw = citationForType(type)
            const n = normalizeCitation(raw)
            expect(n.citationId).toBeTruthy()
            expect(n.sourceId).toBeTruthy()
            expect(n.title).toBeTruthy()
            expect(typeof n.citedText).toBe('string')
            expect(typeof n.isImap).toBe('boolean')
            expect(typeof n.sourceName).toBe('string')
        }
    })
})

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function citationForType(type: TextCitationParam['type']): TextCitationParam {
    switch (type) {
        case 'search_result_location':
            return {
                type: 'search_result_location',
                source: 'synthetic://s',
                title: 'SR',
                cited_text: 'c',
                start_block_index: 0,
                end_block_index: 0,
                search_result_index: 0,
            }
        case 'web_search_result_location':
            return {
                type: 'web_search_result_location',
                url: 'https://example.com',
                title: 'WS',
                cited_text: 'c',
                encrypted_index: 'e',
            }
        case 'char_location':
            return {
                type: 'char_location',
                document_index: 0,
                document_title: 'Doc',
                start_char_index: 0,
                end_char_index: 10,
                cited_text: 'c',
            }
        case 'page_location':
            return {
                type: 'page_location',
                document_index: 0,
                document_title: 'Doc',
                start_page_number: 1,
                end_page_number: 1,
                cited_text: 'c',
            }
        case 'content_block_location':
            return {
                type: 'content_block_location',
                document_index: 0,
                document_title: 'Doc',
                start_block_index: 0,
                end_block_index: 1,
                cited_text: 'c',
            }
    }
    throw new Error(`unexpected type: ${type}`)
}
