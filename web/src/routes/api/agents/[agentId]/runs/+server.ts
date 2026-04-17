import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess, listAgentRuns } from '$lib/server/db/agents.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.db)
    const runs = await listAgentRuns(params.agentId, 50, locals.db)

    // For org agents, strip execution_log
    if (agent.agentType === 'org') {
        return json(runs.map((r) => ({ ...r, executionLog: [] })))
    }

    return json(runs)
}
