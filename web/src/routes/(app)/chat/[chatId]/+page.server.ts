import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getModel } from '$lib/server/db/model-providers.js'
import { getAgent } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'

export const load = async ({ params, locals }) => {
    const chat = await chatRepository.get(params.chatId)
    if (!chat) {
        // throw 404
        error(404, 'Chat not found')
    }

    // Agent chats: fetch agent info and enforce admin access
    let agent: { id: string; name: string; agentType: string } | null = null
    if (chat.agentId) {
        const agentRecord = await getAgent(chat.agentId)
        if (agentRecord?.agentType === 'org' && locals.user?.role !== 'admin') {
            error(403, 'Admin access required')
        }
        if (agentRecord) {
            agent = { id: agentRecord.id, name: agentRecord.name, agentType: agentRecord.agentType }
        }
    }

    const messages = await chatMessageRepository.getByChatId(chat.id)

    let modelDisplayName: string | null = null
    if (chat.modelId) {
        const model = await getModel(chat.modelId)
        if (model) {
            modelDisplayName = model.displayName
        }
    }

    return {
        user: locals.user!,
        chat,
        messages,
        modelDisplayName,
        agent,
    }
}
