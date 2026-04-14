import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess } from '$lib/server/db/agents.js'
import { chatRepository } from '$lib/server/db/chats.js'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.user)

    const chat = await chatRepository.create(
        locals.user.id,
        `Chat with ${agent.name}`,
        agent.modelId ?? undefined,
        agent.id,
    )

    return json({ chatId: chat.id })
}
