import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { sql } from 'drizzle-orm'
import { db } from '$lib/server/db/index.js'
import { ProjectAttachmentRepository, ProjectRepository } from '$lib/server/db/projects.js'

type AttachmentType = 'upload' | 'document'

function isAttachmentType(value: unknown): value is AttachmentType {
    return value === 'upload' || value === 'document'
}

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const project = await new ProjectRepository().get(params.projectId)
    if (!project || project.userId !== locals.user.id) {
        return json({ error: 'Project not found' }, { status: 404 })
    }

    const attachments = await new ProjectAttachmentRepository().listForProject(params.projectId)
    return json({ attachments })
}

export const POST: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const project = await new ProjectRepository().get(params.projectId)
    if (!project || project.userId !== locals.user.id) {
        return json({ error: 'Project not found' }, { status: 404 })
    }

    let body: { attachmentType?: unknown; uploadId?: unknown; documentId?: unknown }
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON body' }, { status: 400 })
    }

    if (!isAttachmentType(body.attachmentType)) {
        return json({ error: 'attachmentType must be "upload" or "document"' }, { status: 400 })
    }

    if (body.attachmentType === 'upload') {
        if (typeof body.uploadId !== 'string' || body.uploadId.length === 0) {
            return json({ error: 'uploadId is required' }, { status: 400 })
        }
        // uploads are user-owned; reject attachments referencing someone else's upload.
        const owned = await db.execute(
            sql`SELECT 1 FROM uploads WHERE id = ${body.uploadId} AND user_id = ${locals.user.id}`,
        )
        if (owned.length === 0) {
            return json({ error: 'Upload not found' }, { status: 404 })
        }
    } else {
        if (typeof body.documentId !== 'string' || body.documentId.length === 0) {
            return json({ error: 'documentId is required' }, { status: 400 })
        }
        const exists = await db.execute(
            sql`SELECT 1 FROM documents d JOIN sources s ON d.source_id = s.id
                WHERE d.id = ${body.documentId} AND s.is_deleted = FALSE`,
        )
        if (exists.length === 0) {
            return json({ error: 'Document not found' }, { status: 404 })
        }
    }

    const attachment = await new ProjectAttachmentRepository().add(params.projectId, {
        type: body.attachmentType,
        uploadId: typeof body.uploadId === 'string' ? body.uploadId : undefined,
        documentId: typeof body.documentId === 'string' ? body.documentId : undefined,
    })

    return json({ attachment, alreadyAttached: attachment === null }, { status: 200 })
}

export const DELETE: RequestHandler = async ({ request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let body: { attachmentId?: unknown }
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON body' }, { status: 400 })
    }
    if (typeof body.attachmentId !== 'string' || body.attachmentId.length === 0) {
        return json({ error: 'attachmentId is required' }, { status: 400 })
    }

    const attachment = await new ProjectAttachmentRepository().getOwned(
        body.attachmentId,
        locals.user.id,
    )
    if (!attachment) {
        return json({ error: 'Attachment not found' }, { status: 404 })
    }

    await new ProjectAttachmentRepository().remove(attachment.id)
    return json({ success: true })
}
