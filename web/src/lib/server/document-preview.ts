import { z } from 'zod'
import type { DocumentPreview, EmailPreviewFields } from '$lib/types/document-preview'

const searcherDocumentSchema = z.object({
    id: z.string(),
    title: z.string(),
    content_type: z.string().nullish(),
    metadata: z
        .object({
            created_at: z.string().nullish(),
            updated_at: z.string().nullish(),
        })
        .nullish(),
    attributes: z.record(z.string(), z.unknown()).nullish(),
})

const searcherResultSchema = z.object({
    document: searcherDocumentSchema,
    highlights: z.array(z.string()),
    match_type: z.string(),
    source_type: z.string().nullish(),
})

const searcherResponseSchema = z.object({
    results: z.array(searcherResultSchema),
})

// Email-like documents (e.g. IMAP threads) expose their headers as attributes.
const emailAttributesSchema = z.object({
    from: z.string(),
    to: z.array(z.string()).optional(),
    cc: z.array(z.string()).optional(),
    folder: z.string().optional(),
    source_name: z.string().optional(),
    message_count: z.number().int().nonnegative().optional(),
})

const FULL_CONTENT_MATCH_TYPE = 'full_content'
// Large documents read without a query come back as the first lines, each
// prefixed with "<line number> | " by the searcher.
const LEADING_LINES_MATCH_TYPE = 'fulltext'
const LINE_NUMBER_PREFIX = /^\d+ \| /

export class DocumentPreviewParseError extends Error {}

function emailFields(attributes: Record<string, unknown> | null | undefined) {
    const parsed = emailAttributesSchema.safeParse(attributes ?? {})
    if (!parsed.success) return null
    const fields: EmailPreviewFields = {
        from: parsed.data.from,
        to: parsed.data.to ?? [],
        cc: parsed.data.cc ?? [],
        account: parsed.data.source_name ?? null,
        folder: parsed.data.folder ?? null,
        messageCount: parsed.data.message_count ?? null,
    }
    return fields
}

function stripLineNumbers(text: string): string {
    return text
        .split('\n')
        .map((line) => line.replace(LINE_NUMBER_PREFIX, ''))
        .join('\n')
}

/**
 * Converts the searcher's read-by-document-id response into a preview.
 * Returns null when the searcher found no readable content for the document.
 */
export function parseDocumentPreview(payload: unknown): DocumentPreview | null {
    const parsed = searcherResponseSchema.safeParse(payload)
    if (!parsed.success) {
        throw new DocumentPreviewParseError(`Invalid searcher response: ${parsed.error.message}`)
    }

    const [result] = parsed.data.results
    if (!result) return null

    let content: string
    let isTruncated: boolean
    if (result.match_type === FULL_CONTENT_MATCH_TYPE) {
        content = result.highlights.join('\n')
        isTruncated = false
    } else if (result.match_type === LEADING_LINES_MATCH_TYPE) {
        content = stripLineNumbers(result.highlights.join('\n'))
        isTruncated = true
    } else {
        throw new DocumentPreviewParseError(`Unexpected match type: ${result.match_type}`)
    }

    const { document } = result
    return {
        id: document.id,
        title: document.title,
        sourceType: result.source_type ?? null,
        contentType: document.content_type ?? null,
        date: document.metadata?.updated_at ?? document.metadata?.created_at ?? null,
        content,
        isTruncated,
        email: emailFields(document.attributes),
    }
}
