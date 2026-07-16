import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { SkillRepository } from '$lib/server/db/skills.js'
import { createSkillSchema, type CreateSkillInput } from '$lib/skills.js'

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const repo = new SkillRepository()
    const skills = await repo.listVisible(locals.user.id)
    return json({ skills })
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let parsed: CreateSkillInput
    try {
        parsed = createSkillSchema.parse(await request.json())
    } catch {
        return json(
            { error: 'Invalid skill payload. name, description, and instructions are required.' },
            { status: 400 },
        )
    }

    try {
        const repo = new SkillRepository()
        const skill = await repo.create({
            userId: locals.user.id,
            name: parsed.name,
            description: parsed.description,
            instructions: parsed.instructions,
            visibility: parsed.visibility,
        })
        return json(skill, { status: 201 })
    } catch {
        return json({ error: 'Failed to create skill' }, { status: 500 })
    }
}
