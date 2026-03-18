import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { getAgent, listAgentRuns } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'
import { listAllActiveModels } from '$lib/server/db/model-providers.js'

export const load: PageServerLoad = async ({ locals, params }) => {
    const { user } = requireActiveUser(locals)

    const agent = await getAgent(params.agentId)
    if (!agent) {
        throw error(404, 'Agent not found')
    }
    if (agent.agentType !== 'org' && agent.userId !== user.id) {
        throw error(403, 'Access denied')
    }
    if (agent.agentType === 'org' && user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const [runs, models] = await Promise.all([listAgentRuns(params.agentId), listAllActiveModels()])

    return { user, agent, runs, models }
}
