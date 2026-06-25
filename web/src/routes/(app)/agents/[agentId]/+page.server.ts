import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { requireAgentAccess, listAgentRuns, getActiveAgentRun } from '$lib/server/db/agents.js'
import { listAllActiveModels } from '$lib/server/db/model-providers.js'

export const load: PageServerLoad = async ({ locals, params }) => {
    const { user } = requireActiveUser(locals)
    const agent = await requireAgentAccess(params.agentId, user)

    const [runs, models, activeRun] = await Promise.all([
        listAgentRuns(params.agentId),
        listAllActiveModels(),
        getActiveAgentRun(params.agentId),
    ])

    return { user, agent, runs, models, activeRun }
}
