import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { requireAgentAccess, getAgentRun } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'

export const load: PageServerLoad = async ({ locals, params }) => {
    const { user } = requireActiveUser(locals)
    const agent = await requireAgentAccess(params.agentId, locals.db)

    const run = await getAgentRun(params.runId, locals.db)
    if (!run || run.agentId !== params.agentId) {
        throw error(404, 'Run not found')
    }

    // For org agents, strip execution_log from the response
    const sanitizedRun = agent.agentType === 'org' ? { ...run, executionLog: [] } : run

    return { user, agent, run: sanitizedRun }
}
