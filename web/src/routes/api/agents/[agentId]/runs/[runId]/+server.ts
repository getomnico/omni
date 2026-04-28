import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess, getAgentRun } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.db)

    const run = await getAgentRun(params.runId, locals.db)
    if (!run || run.agentId !== params.agentId) {
        throw error(404, 'Run not found')
    }

    // For org agents, strip execution_log
    if (agent.agentType === 'org') {
        return json({ ...run, executionLog: [] })
    }

    return json(run)
}
