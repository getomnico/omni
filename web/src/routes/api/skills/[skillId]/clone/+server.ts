import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { SkillRepository } from '$lib/server/db/skills.js'
import { cloneSkillSchema } from '$lib/skills.js'

export const POST: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    let payload: unknown = {}
    try {
        payload = await request.json()
    } catch {
        payload = {}
    }
    if (!cloneSkillSchema.safeParse(payload).success) {
        return json({ error: 'Invalid clone payload.' }, { status: 400 })
    }

    const repo = new SkillRepository()
    const cloned = await repo.clone(params.skillId, locals.user.id)

    if (!cloned) {
        return json({ error: 'Skill not found or not accessible' }, { status: 404 })
    }

    return json(cloned, { status: 201 })
}
