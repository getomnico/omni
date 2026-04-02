import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess } from '$lib/server/db/agents.js'
import { chatRepository } from '$lib/server/db/chats.js'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    if (locals.user.role !== 'admin') {
        return json({ error: 'Admin access required' }, { status: 403 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.user)

    if (agent.agentType !== 'org') {
        return json({ error: 'Chat is only supported for org-level agents' }, { status: 400 })
    }

    const chat = await chatRepository.create(
        locals.user.id,
        `Chat with ${agent.name}`,
        agent.modelId ?? undefined,
        agent.id,
    )

    return json({ chatId: chat.id })
}
