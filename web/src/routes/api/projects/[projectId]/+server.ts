import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { ProjectNameTakenError, ProjectRepository } from '$lib/server/db/projects.js'

async function getOwnedProject(projectId: string, userId: string) {
    const project = await new ProjectRepository().get(projectId)
    if (!project || project.userId !== userId) {
        return null
    }
    return project
}

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const project = await getOwnedProject(params.projectId, locals.user.id)
    if (!project) {
        return json({ error: 'Project not found' }, { status: 404 })
    }

    return json({ project })
}

export const PATCH: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const project = await getOwnedProject(params.projectId, locals.user.id)
    if (!project) {
        return json({ error: 'Project not found' }, { status: 404 })
    }

    let body: {
        name?: unknown
        description?: unknown
        instructions?: unknown
        isArchived?: unknown
    }
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON body' }, { status: 400 })
    }

    const updates: {
        name?: string
        description?: string | null
        instructions?: string | null
        isArchived?: boolean
    } = {}

    if (body.name !== undefined) {
        if (typeof body.name !== 'string' || body.name.trim().length === 0) {
            return json({ error: 'Project name cannot be empty' }, { status: 400 })
        }
        if (body.name.trim().length > 200) {
            return json({ error: 'Project name is too long' }, { status: 400 })
        }
        updates.name = body.name.trim()
    }
    if (body.description !== undefined) {
        if (body.description !== null && typeof body.description !== 'string') {
            return json({ error: 'Invalid description' }, { status: 400 })
        }
        updates.description =
            typeof body.description === 'string' && body.description.trim().length > 0
                ? body.description.trim()
                : null
    }
    if (body.instructions !== undefined) {
        if (body.instructions !== null && typeof body.instructions !== 'string') {
            return json({ error: 'Invalid instructions' }, { status: 400 })
        }
        updates.instructions =
            typeof body.instructions === 'string' && body.instructions.trim().length > 0
                ? body.instructions.trim()
                : null
    }
    if (body.isArchived !== undefined) {
        if (typeof body.isArchived !== 'boolean') {
            return json({ error: 'Invalid isArchived' }, { status: 400 })
        }
        updates.isArchived = body.isArchived
    }

    try {
        const updated = await new ProjectRepository().update(params.projectId, updates)
        if (!updated) {
            return json({ error: 'Project not found' }, { status: 404 })
        }
        return json({ project: updated })
    } catch (error) {
        if (error instanceof ProjectNameTakenError) {
            return json({ error: error.message }, { status: 409 })
        }
        throw error
    }
}

export const DELETE: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const project = await getOwnedProject(params.projectId, locals.user.id)
    if (!project) {
        return json({ error: 'Project not found' }, { status: 404 })
    }

    const deleted = await new ProjectRepository().delete(params.projectId)
    if (!deleted) {
        return json({ error: 'Project not found' }, { status: 404 })
    }
    return json({ success: true })
}
