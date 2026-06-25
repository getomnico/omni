import type { PageServerLoad } from './$types.js'
import { requireAuth, requireActiveUser } from '$lib/server/authHelpers.js'
import { listAgents, listActiveRunsForAgents } from '$lib/server/db/agents.js'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireActiveUser(locals)
    const agents = await listAgents(user.id)
    const activeRuns = await listActiveRunsForAgents(agents.map((agent) => agent.id))
    return { user, agents, activeRuns: Object.fromEntries(activeRuns) }
}
