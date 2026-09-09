import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { ProjectRepository } from '$lib/server/db/projects.js'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireActiveUser(locals)
    const projects = await new ProjectRepository().listWithChatCounts(user.id)

    return {
        user,
        projects,
    }
}
