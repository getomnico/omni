import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { ProjectNameTakenError, ProjectRepository } from '$lib/server/db/projects.js'

function parseOptionalString(value: unknown): string | undefined {
    if (value === undefined) return undefined
    if (typeof value !== 'string') return undefined
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : undefined
}

export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const includeArchived = url.searchParams.get('includeArchived') !== 'false'
    const projects = await new ProjectRepository().listWithChatCounts(
        locals.user.id,
        includeArchived,
    )
    return json({ projects })
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let body: { name?: unknown; description?: unknown; instructions?: unknown }
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON body' }, { status: 400 })
    }

    const name = parseOptionalString(body.name)
    if (!name) {
        return json({ error: 'Project name is required' }, { status: 400 })
    }
    if (name.length > 200) {
        return json({ error: 'Project name is too long' }, { status: 400 })
    }

    const description = parseOptionalString(body.description)
    const instructions = parseOptionalString(body.instructions)

    try {
        const project = await new ProjectRepository().create(
            locals.user.id,
            name,
            description,
            instructions,
        )
        return json({ project }, { status: 201 })
    } catch (error) {
        if (error instanceof ProjectNameTakenError) {
            return json({ error: error.message }, { status: 409 })
        }
        throw error
    }
}
