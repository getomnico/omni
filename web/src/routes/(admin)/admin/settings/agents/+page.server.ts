import type { PageServerLoad } from './$types.js'
import { requireAdmin, requireActiveUser } from '$lib/server/authHelpers.js'
import { listActiveRunsForAgents, listOrgAgents } from '$lib/server/db/agents.js'

export const load: PageServerLoad = async ({ locals }) => {
    requireActiveUser(locals)
    const { user } = requireAdmin(locals)
    const agents = await listOrgAgents()
    const activeRuns = await listActiveRunsForAgents(agents.map((agent) => agent.id))
    return { user, agents, activeRuns: Object.fromEntries(activeRuns) }
}
