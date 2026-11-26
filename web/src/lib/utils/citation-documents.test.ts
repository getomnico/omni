import { describe, expect, it } from 'vitest'
import type {
    ContentBlockParam,
    MessageParam,
    TextCitationParam,
} from '@anthropic-ai/sdk/resources/messages'
import { buildCitationDocumentIndex, resolveCitationDocumentId } from './citation-documents'

const IMAP_SOURCE = 'imap:Work / INBOX / 2026-01-15 / Quarterly numbers'

function searchResultCitation(source: string, title: string): TextCitationParam {
    return {
        type: 'search_result_location',
        source,
        title,
        cited_text: 'Revenue grew.',
        search_result_index: 0,
        start_block_index: 0,
        end_block_index: 0,
    }
}

function documentCitation(title: string): TextCitationParam {
    return {
        type: 'char_location',
        document_index: 0,
        document_title: title,
        start_char_index: 0,
        end_char_index: 0,
        cited_text: 'Revenue grew.',
    }
}

function searchToolResult(
    toolUseId: string,
    results: Array<{ source: string; title: string; documentId?: string; ref?: string }>,
): MessageParam {
    return {
        role: 'user',
        content: [
            {
                type: 'tool_result',
                tool_use_id: toolUseId,
                content: results.map((result) => ({
                    type: 'search_result' as const,
                    source: result.source,
                    title: result.title,
                    ...(result.documentId ? { document_id: result.documentId } : {}),
                    content: result.ref
                        ? [{ type: 'text' as const, text: `[_ref:${result.ref}]` }]
                        : [],
                })),
            },
        ],
    }
}

function readDocumentCall(toolUseId: string, id: string): MessageParam {
    return {
        role: 'assistant',
        content: [{ type: 'tool_use', id: toolUseId, name: 'read_document', input: { id } }],
    }
}

function readDocumentResult(toolUseId: string, title: string): MessageParam {
    return {
        role: 'user',
        content: [
            {
                type: 'tool_result',
                tool_use_id: toolUseId,
                content: [
                    {
                        type: 'document',
                        title,
                        source: { type: 'text', media_type: 'text/plain', data: 'body' },
                    },
                ],
            },
        ],
    }
}

describe('resolveCitationDocumentId', () => {
    it('resolves a search result citation via the structured document_id', () => {
        const index = buildCitationDocumentIndex([
            searchToolResult('toolu_1', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', documentId: 'DOC_A' },
                {
                    source: 'imap:Work / INBOX / 2026-01-16 / Other',
                    title: 'Other',
                    documentId: 'DOC_B',
                },
            ]),
        ])

        expect(
            resolveCitationDocumentId(
                index,
                searchResultCitation(IMAP_SOURCE, 'Quarterly numbers'),
            ),
        ).toBe('DOC_A')
    })

    it('falls back to the [_ref:ID] marker of results persisted without document_id', () => {
        const index = buildCitationDocumentIndex([
            searchToolResult('toolu_1', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', ref: 'DOC_LEGACY' },
            ]),
        ])

        expect(
            resolveCitationDocumentId(
                index,
                searchResultCitation(IMAP_SOURCE, 'Quarterly numbers'),
            ),
        ).toBe('DOC_LEGACY')
    })

    it('refuses to guess when the same source and title map to different documents', () => {
        const index = buildCitationDocumentIndex([
            searchToolResult('toolu_1', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', documentId: 'DOC_A' },
            ]),
            searchToolResult('toolu_2', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', documentId: 'DOC_B' },
            ]),
        ])

        expect(
            resolveCitationDocumentId(
                index,
                searchResultCitation(IMAP_SOURCE, 'Quarterly numbers'),
            ),
        ).toBeNull()
    })

    it('keeps a mapping when repeated searches return the same document', () => {
        const index = buildCitationDocumentIndex([
            searchToolResult('toolu_1', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', documentId: 'DOC_A' },
            ]),
            searchToolResult('toolu_2', [
                { source: IMAP_SOURCE, title: 'Quarterly numbers', documentId: 'DOC_A' },
            ]),
        ])

        expect(
            resolveCitationDocumentId(
                index,
                searchResultCitation(IMAP_SOURCE, 'Quarterly numbers'),
            ),
        ).toBe('DOC_A')
    })

    it('resolves read_document citations through the tool call input', () => {
        const index = buildCitationDocumentIndex([
            readDocumentCall('toolu_read', '_ref:DOC_READ'),
            readDocumentResult('toolu_read', 'Quarterly numbers'),
        ])

        expect(resolveCitationDocumentId(index, documentCitation('Quarterly numbers'))).toBe(
            'DOC_READ',
        )
    })

    it('does not resolve document blocks that did not come from read_document', () => {
        const index = buildCitationDocumentIndex([
            {
                role: 'assistant',
                content: [
                    { type: 'tool_use', id: 'toolu_other', name: 'read_file', input: { id: 'X' } },
                ],
            },
            readDocumentResult('toolu_other', 'Quarterly numbers'),
        ])

        expect(resolveCitationDocumentId(index, documentCitation('Quarterly numbers'))).toBeNull()
    })

    it('resolves citations of mentioned documents', () => {
        const index = buildCitationDocumentIndex([
            {
                role: 'user',
                content: [
                    { type: 'text', text: 'Summarize this mail' },
                    {
                        type: 'document',
                        source: {
                            type: 'omni_mention',
                            document_id: 'DOC_MENTION',
                            title: 'Quarterly numbers',
                        },
                    } as unknown as ContentBlockParam,
                ],
            },
        ])

        expect(resolveCitationDocumentId(index, documentCitation('Quarterly numbers'))).toBe(
            'DOC_MENTION',
        )
    })

    it('refuses to guess when a mention and a read_document result share a title', () => {
        const index = buildCitationDocumentIndex([
            {
                role: 'user',
                content: [
                    {
                        type: 'document',
                        source: {
                            type: 'omni_mention',
                            document_id: 'DOC_MENTION',
                            title: 'Quarterly numbers',
                        },
                    } as unknown as ContentBlockParam,
                ],
            },
            readDocumentCall('toolu_read', 'DOC_READ'),
            readDocumentResult('toolu_read', 'Quarterly numbers'),
        ])

        expect(resolveCitationDocumentId(index, documentCitation('Quarterly numbers'))).toBeNull()
    })

    it('returns null for unknown citations and web results', () => {
        const index = buildCitationDocumentIndex([])

        expect(
            resolveCitationDocumentId(
                index,
                searchResultCitation(IMAP_SOURCE, 'Quarterly numbers'),
            ),
        ).toBeNull()
        expect(
            resolveCitationDocumentId(index, {
                type: 'web_search_result_location',
                url: 'https://example.com',
                title: 'Example',
                encrypted_index: 'abc',
                cited_text: 'Example',
            }),
        ).toBeNull()
    })
})
