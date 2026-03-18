import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { getAgent, listAgentRuns } from '$lib/server/db/agents.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await getAgent(params.agentId)
    if (!agent) {
        return json({ error: 'Agent not found' }, { status: 404 })
    }

    if (agent.agentType === 'org') {
        if (locals.user.role !== 'admin') {
            return json({ error: 'Admin access required' }, { status: 403 })
        }
    } else if (agent.userId !== locals.user.id) {
        return json({ error: 'Access denied' }, { status: 403 })
    }

    const runs = await listAgentRuns(params.agentId)

    // For org agents, strip execution_log
    if (agent.agentType === 'org') {
        return json(runs.map((r) => ({ ...r, executionLog: [] })))
    }

    return json(runs)
}
