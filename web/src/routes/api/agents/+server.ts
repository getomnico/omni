import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { createAgent, listAgents, listOrgAgents } from '$lib/server/db/agents.js'

export const GET: RequestHandler = async ({ locals, url }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const type = url.searchParams.get('type')

    if (type === 'org') {
        if (locals.user.role !== 'admin') {
            return json({ error: 'Admin access required' }, { status: 403 })
        }
        const agents = await listOrgAgents(locals.db)
        return json(agents)
    }

    const agents = await listAgents(locals.db)
    return json(agents)
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const data = await request.json()

    if (!data.name || !data.instructions || !data.scheduleType || !data.scheduleValue) {
        return json({ error: 'Missing required fields' }, { status: 400 })
    }

    // Only admins can create org agents
    if (data.agentType === 'org' && locals.user.role !== 'admin') {
        return json({ error: 'Admin access required for org agents' }, { status: 403 })
    }

    try {
        const agent = await createAgent(
            {
                userId: locals.user.id,
                name: data.name,
                instructions: data.instructions,
                agentType: data.agentType || 'user',
                scheduleType: data.scheduleType,
                scheduleValue: data.scheduleValue,
                modelId: data.modelId,
                allowedSources: data.allowedSources,
                allowedActions: data.allowedActions,
            },
            locals.db,
        )
        return json(agent)
    } catch (error) {
        return json(
            {
                error: 'Failed to create agent',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
