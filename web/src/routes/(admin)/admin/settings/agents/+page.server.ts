import type { PageServerLoad } from './$types.js'
import { requireAdmin, requireActiveUser } from '$lib/server/authHelpers.js'
import { listActiveRunsForAgents, listOrgAgents } from '$lib/server/db/agents.js'
import { listAllActiveModels } from '$lib/server/db/model-providers.js'

export const load: PageServerLoad = async ({ locals }) => {
    requireActiveUser(locals)
    const { user } = requireAdmin(locals)
    const agents = await listOrgAgents()
    const activeRuns = await listActiveRunsForAgents(agents.map((agent) => agent.id))
    const models = await listAllActiveModels()
    return { user, agents, activeRuns: Object.fromEntries(activeRuns), models }
}
