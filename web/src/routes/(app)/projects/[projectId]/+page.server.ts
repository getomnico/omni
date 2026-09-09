import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { error } from '@sveltejs/kit'
import { sql } from 'drizzle-orm'
import { db } from '$lib/server/db/index.js'
import { ChatRepository } from '$lib/server/db/chats.js'
import { ProjectAttachmentRepository, ProjectRepository } from '$lib/server/db/projects.js'

export const load: PageServerLoad = async ({ locals, params }) => {
    const { user } = requireActiveUser(locals)

    const project = await new ProjectRepository().get(params.projectId)
    if (!project || project.userId !== user.id) {
        error(404, 'Project not found')
    }

    const [attachments, chats] = await Promise.all([
        new ProjectAttachmentRepository().listForProject(project.id),
        new ChatRepository().getByUserId(user.id, { projectId: project.id, limit: 50 }),
    ])

    // Resolve document titles for display. Content/permission enforcement stays
    // with search/read_document; here we only need a human-readable label.
    const documentIds = attachments
        .filter((a) => a.attachmentType === 'document' && a.documentId)
        .map((a) => a.documentId as string)

    let documentsByUlid: Record<string, { title: string | null; contentId: string | null }> = {}
    if (documentIds.length > 0) {
        const rows = await db.execute<{
            id: string
            title: string | null
            content_id: string | null
        }>(
            sql`SELECT d.id, d.title, d.content_id
                FROM documents d
                JOIN sources s ON d.source_id = s.id
                WHERE d.id = ANY(${documentIds})
                  AND s.is_deleted = FALSE`,
        )
        documentsByUlid = Object.fromEntries(
            rows.map((row) => [row.id, { title: row.title, contentId: row.content_id }]),
        )
    }

    const uploadIds = attachments
        .filter((a) => a.attachmentType === 'upload' && a.uploadId)
        .map((a) => a.uploadId as string)
    let uploadsById: Record<string, { filename: string }> = {}
    if (uploadIds.length > 0) {
        const rows = await db.execute<{ id: string; filename: string }>(
            sql`SELECT id, filename FROM uploads WHERE id = ANY(${uploadIds})`,
        )
        uploadsById = Object.fromEntries(rows.map((row) => [row.id, { filename: row.filename }]))
    }

    return {
        user,
        project,
        chats,
        attachments: attachments.map((attachment) => ({
            id: attachment.id,
            attachmentType: attachment.attachmentType,
            uploadId: attachment.uploadId,
            documentId: attachment.documentId,
            addedAt: attachment.addedAt,
            title:
                attachment.attachmentType === 'document' && attachment.documentId
                    ? (documentsByUlid[attachment.documentId]?.title ?? 'Unavailable document')
                    : attachment.uploadId
                      ? (uploadsById[attachment.uploadId]?.filename ?? 'Unavailable file')
                      : null,
        })),
    }
}
