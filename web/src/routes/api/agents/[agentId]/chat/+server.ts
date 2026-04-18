import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { requireAgentAccess } from '$lib/server/db/agents.js'
import { ChatRepository } from '$lib/server/db/chats.js'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user?.id) {
        return json({ error: 'User not authenticated' }, { status: 401 })
    }

    const agent = await requireAgentAccess(params.agentId, locals.db)

    const chatRepo = new ChatRepository(locals.db)
    const chat = await chatRepo.create(
        locals.user.id,
        `Chat with ${agent.name}`,
        agent.modelId ?? undefined,
        agent.id,
    )

    return json({ chatId: chat.id })
}
