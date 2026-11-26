import { z } from 'zod'

export const emailPreviewFieldsSchema = z.object({
    from: z.string(),
    to: z.array(z.string()),
    cc: z.array(z.string()),
    account: z.string().nullable(),
    folder: z.string().nullable(),
    messageCount: z.number().int().nullable(),
})

/** Stored document returned by `GET /api/documents/[id]` for in-app previews. */
export const documentPreviewSchema = z.object({
    id: z.string(),
    title: z.string(),
    sourceType: z.string().nullable(),
    contentType: z.string().nullable(),
    /** Original content date from the connector (RFC 3339), not the index timestamp. */
    date: z.string().nullable(),
    content: z.string(),
    /** True when only the beginning of a large document was returned. */
    isTruncated: z.boolean(),
    email: emailPreviewFieldsSchema.nullable(),
})

export type EmailPreviewFields = z.infer<typeof emailPreviewFieldsSchema>
export type DocumentPreview = z.infer<typeof documentPreviewSchema>
