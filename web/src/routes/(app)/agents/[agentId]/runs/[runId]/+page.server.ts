import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { getAgent, getAgentRun } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'

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

    const run = await getAgentRun(params.runId)
    if (!run || run.agentId !== params.agentId) {
        throw error(404, 'Run not found')
    }

    // For org agents, strip execution_log from the response
    const sanitizedRun = agent.agentType === 'org' ? { ...run, executionLog: [] } : run

    return { user, agent, run: sanitizedRun }
}
