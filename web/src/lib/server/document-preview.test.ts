import { describe, expect, it } from 'vitest'
import { DocumentPreviewParseError, parseDocumentPreview } from './document-preview'

function searcherResponse(result: Record<string, unknown>) {
    return { results: [result], total_count: 1, query_time_ms: 1, has_more: false, query: '' }
}

const imapDocument = {
    id: 'DOC_A',
    title: 'Quarterly numbers',
    content_type: 'email_thread',
    url: null,
    metadata: {
        created_at: '2026-01-14T09:00:00Z',
        updated_at: '2026-01-15T10:30:00Z',
        extra: { folder: 'INBOX' },
    },
    attributes: {
        from: 'Alice <alice@example.com>',
        to: ['bob@example.com'],
        cc: [],
        folder: 'INBOX',
        source_name: 'Work',
        message_count: 2,
        subject: 'Quarterly numbers',
    },
}

describe('parseDocumentPreview', () => {
    it('maps a full email thread including its headers', () => {
        const preview = parseDocumentPreview(
            searcherResponse({
                document: imapDocument,
                highlights: ['From: Alice\n\nRevenue grew.'],
                match_type: 'full_content',
                source_type: 'imap',
            }),
        )

        expect(preview).toEqual({
            id: 'DOC_A',
            title: 'Quarterly numbers',
            sourceType: 'imap',
            contentType: 'email_thread',
            date: '2026-01-15T10:30:00Z',
            content: 'From: Alice\n\nRevenue grew.',
            isTruncated: false,
            email: {
                from: 'Alice <alice@example.com>',
                to: ['bob@example.com'],
                cc: [],
                account: 'Work',
                folder: 'INBOX',
                messageCount: 2,
            },
        })
    })

    it('strips line numbers and flags truncation for large documents', () => {
        const preview = parseDocumentPreview(
            searcherResponse({
                document: { ...imapDocument, attributes: {} },
                highlights: ['1 | First line\n2 | 3 | literal pipe'],
                match_type: 'fulltext',
                source_type: 'local_files',
            }),
        )

        expect(preview?.content).toBe('First line\n3 | literal pipe')
        expect(preview?.isTruncated).toBe(true)
        expect(preview?.email).toBeNull()
    })

    it('returns null when the searcher found no content', () => {
        expect(parseDocumentPreview({ results: [] })).toBeNull()
    })

    it('rejects malformed searcher responses', () => {
        expect(() => parseDocumentPreview({ results: [{ document: { id: 1 } }] })).toThrow(
            DocumentPreviewParseError,
        )
        expect(() =>
            parseDocumentPreview(
                searcherResponse({
                    document: imapDocument,
                    highlights: ['chunk'],
                    match_type: 'hybrid',
                    source_type: 'imap',
                }),
            ),
        ).toThrow(DocumentPreviewParseError)
    })
})
