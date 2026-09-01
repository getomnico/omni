import { describe, expect, it } from 'vitest'
import { citationIdFromCitation } from '$lib/utils/citations'
import { preprocessCitationPlaceholders } from './citations'

const citation = {
    type: 'search_result_location' as const,
    source: 'imap://example/message-1',
    title: 'Example message',
    cited_text: 'Example excerpt',
    start_block_index: 0,
    end_block_index: 0,
    search_result_index: 0,
}

const otherCitation = {
    type: 'search_result_location' as const,
    source: 'imap://example/message-2',
    title: 'Other message',
    cited_text: 'Other excerpt',
    start_block_index: 0,
    end_block_index: 0,
    search_result_index: 1,
}

describe('preprocessCitationPlaceholders', () => {
    it('replaces a known citation with its indexed placeholder', () => {
        const id = encodeURIComponent(citationIdFromCitation(citation))

        expect(preprocessCitationPlaceholders(`before {omni-cit:${id}} after`, [citation])).toBe(
            'before <span class="omni-reflink" data-citation-idx="0"></span> after',
        )
    })

    it('uses the citation index from the supplied order', () => {
        const id = encodeURIComponent(citationIdFromCitation(citation))

        expect(preprocessCitationPlaceholders(`{omni-cit:${id}}`, [otherCitation, citation])).toBe(
            '<span class="omni-reflink" data-citation-idx="1"></span>',
        )
    })

    it('preserves unknown and malformed placeholders', () => {
        const unknownId = encodeURIComponent('unknown-citation')

        expect(preprocessCitationPlaceholders(`{omni-cit:${unknownId}}`, [citation])).toBe(
            `{omni-cit:${unknownId}}`,
        )
        expect(preprocessCitationPlaceholders('{omni-cit:%invalid}', [citation])).toBe(
            '{omni-cit:%invalid}',
        )
        expect(preprocessCitationPlaceholders('{omni-cit:missing}', undefined)).toBe(
            '{omni-cit:missing}',
        )
    })
})
