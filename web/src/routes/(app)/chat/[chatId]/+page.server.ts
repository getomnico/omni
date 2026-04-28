import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getModel } from '$lib/server/db/model-providers.js'
import { getAgent } from '$lib/server/db/agents.js'
import { error } from '@sveltejs/kit'
import type { ChatMessage } from '$lib/server/db/schema.js'

function collectUploadIds(messages: ChatMessage[]): Set<string> {
    const ids = new Set<string>()
    for (const msg of messages) {
        const content = msg.message.content
        if (typeof content === 'string') continue
        for (const block of content) {
            if (
                (block.type === 'document' || block.type === 'image') &&
                'source' in block &&
                (block.source as { type: string }).type === 'omni_upload'
            ) {
                ids.add((block.source as { upload_id: string }).upload_id)
            }
        }
    }
    return ids
}

async function resolveUploadFilenames(
    ids: Iterable<string>,
    fetch: typeof globalThis.fetch,
): Promise<Record<string, string>> {
    const result: Record<string, string> = {}
    const lookups = Array.from(ids).map(async (id) => {
        try {
            const resp = await fetch(`/api/uploads/${id}`)
            if (!resp.ok) return
            const upload = (await resp.json()) as { filename: string }
            result[id] = upload.filename
        } catch {
            // Swallow — unresolved IDs fall back client-side.
        }
    })
    await Promise.all(lookups)
    return result
}

export const load = async ({ params, locals, fetch }) => {
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

    const uploadIds = collectUploadIds(messages)
    const uploadFilenames = await resolveUploadFilenames(uploadIds, fetch)

    return {
        user: locals.user!,
        chat,
        messages,
        modelDisplayName,
        agent,
        uploadFilenames,
    }
}
