import type { PageServerLoad } from './$types.js'
import { requireAuth, requireActiveUser } from '$lib/server/authHelpers.js'
import { listAgents } from '$lib/server/db/agents.js'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireActiveUser(locals)
    const agents = await listAgents(locals.db)
    return { user, agents }
}
