import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { SkillRepository } from '$lib/server/db/skills.js'
import { services } from '$lib/server/config.js'
import { updateSkillSchema, type UpdateSkillInput } from '$lib/skills.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const repo = new SkillRepository()
    const skill = await repo.getVisibleById(params.skillId, locals.user.id)
    if (!skill) {
        return json({ error: 'Skill not found' }, { status: 404 })
    }

    return json(skill)
}

export const PUT: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const repo = new SkillRepository()

    // Verify visibility first — 404 if invisible
    const existing = await repo.getVisibleById(params.skillId, locals.user.id)
    if (!existing) {
        return json({ error: 'Skill not found' }, { status: 404 })
    }

    // 403 if visible but not owned
    if (existing.ownerId !== locals.user.id) {
        return json({ error: 'Only the owner can update this skill' }, { status: 403 })
    }

    let parsed: UpdateSkillInput
    try {
        parsed = updateSkillSchema.parse(await request.json())
    } catch {
        return json(
            { error: 'Invalid skill payload. Provide at least one field to update.' },
            { status: 400 },
        )
    }

    try {
        const updated = await repo.update(params.skillId, locals.user.id, parsed)
        if (!updated) {
            return json({ error: 'Skill not found' }, { status: 404 })
        }

        return json(updated)
    } catch {
        return json({ error: 'Failed to update skill' }, { status: 500 })
    }
}

async function pruneSkillCapability(skillId: string, logger: App.Locals['logger']) {
    try {
        const response = await fetch(`${services.searcherUrl}/capabilities/sync`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                publisher_id: `omni:skill-library:${skillId}`,
                capability_type: 'skill',
                capabilities: [],
            }),
        })
        if (!response.ok) {
            logger.warn('Failed to prune deleted skill capability', undefined, {
                skillId,
                status: response.status,
            })
        }
    } catch (error) {
        logger.warn('Failed to prune deleted skill capability', error as Error, { skillId })
    }
}

export const DELETE: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const repo = new SkillRepository()

    // Check ownership
    const existing = await repo.getVisibleById(params.skillId, locals.user.id)
    if (!existing) {
        return json({ error: 'Skill not found' }, { status: 404 })
    }

    if (existing.ownerId !== locals.user.id) {
        return json({ error: 'Only the owner can delete this skill' }, { status: 403 })
    }

    const deleted = await repo.delete(params.skillId, locals.user.id)
    if (!deleted) {
        return json({ error: 'Skill not found' }, { status: 404 })
    }

    await pruneSkillCapability(params.skillId, locals.logger)

    return json({ success: true })
}
