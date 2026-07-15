import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { SkillRepository } from '$lib/server/db/skills.js'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireActiveUser(locals)
    const repo = new SkillRepository()
    const skills = await repo.listVisible(user.id)

    return {
        user,
        skills,
    }
}
