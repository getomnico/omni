import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess } from '$lib/server/db/agents.js'
import { getConfig } from '$lib/server/config.js'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    await requireAgentAccess(params.agentId, locals.db)

    const config = getConfig()
    const response = await fetch(
        `${config.services.aiServiceUrl}/agents/${params.agentId}/trigger`,
        {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'x-user-id': locals.user.id,
            },
        },
    )

    if (!response.ok) {
        return json({ error: 'Failed to trigger agent' }, { status: response.status })
    }

    const result = await response.json()
    return json(result)
}
