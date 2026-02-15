import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getModel } from '$lib/server/db/model-providers.js'
import { error } from '@sveltejs/kit'

export const load = async ({ params, locals }) => {
    const chat = await chatRepository.get(params.chatId)
    if (!chat) {
        // throw 404
        error(404, 'Chat not found')
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
    }
}
