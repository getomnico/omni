import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess, updateAgent, deleteAgent } from '$lib/server/db/agents.js'

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.db)
    return json(agent)
}

export const PUT: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    await requireAgentAccess(params.agentId, locals.db)

    const data = await request.json()
    const updated = await updateAgent(params.agentId, data, locals.db)
    return json(updated)
}

export const DELETE: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    await requireAgentAccess(params.agentId, locals.db)
    await deleteAgent(params.agentId, locals.db)
    return json({ success: true })
}
