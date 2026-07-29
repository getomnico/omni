import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getModel } from '$lib/server/db/model-providers.js'
import { getAgent } from '$lib/server/db/agents.js'
import { toolApprovalRepository } from '$lib/server/db/tool-approvals.js'
import { error } from '@sveltejs/kit'
import type { ChatMessage } from '$lib/server/db/schema.js'

function collectActiveUnansweredToolCallIds(messages: ChatMessage[]): Set<string> {
    const toolCallIds = new Set<string>()
    const answeredIds = new Set<string>()
    for (const msg of messages) {
        const content = msg.message.content
        if (!Array.isArray(content)) continue
        for (const block of content) {
            if (block.type === 'tool_use') {
                toolCallIds.add(block.id)
            } else if (block.type === 'tool_result') {
                answeredIds.add(block.tool_use_id)
            }
        }
    }
    return new Set([...toolCallIds].filter((toolCallId) => !answeredIds.has(toolCallId)))
}

type OmniUploadSource = {
    type: 'omni_upload'
    upload_id: string
}

function isOmniUploadSource(source: unknown): source is OmniUploadSource {
    if (typeof source !== 'object' || source === null) return false

    const candidate = source as Record<string, unknown>
    return candidate.type === 'omni_upload' && typeof candidate.upload_id === 'string'
}

function collectUploadIds(messages: ChatMessage[]): Set<string> {
    const ids = new Set<string>()
    for (const msg of messages) {
        const content = msg.message.content
        if (typeof content === 'string') continue
        for (const block of content) {
            const source: unknown = 'source' in block ? block.source : null
            if (
                (block.type === 'document' || block.type === 'image') &&
                isOmniUploadSource(source)
            ) {
                ids.add(source.upload_id)
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

export const load = async ({ params, locals, fetch, depends }) => {
    if (!locals.user?.id) {
        error(401, 'Authentication required')
    }

    const chat = await chatRepository.get(params.chatId)
    if (!chat) {
        error(404, 'Chat not found')
    }

    if (chat.userId !== locals.user.id) {
        error(403, 'Forbidden')
    }

    depends(`app:chat:${params.chatId}`)

    let agent: { id: string; name: string; agentType: string } | null = null
    if (chat.agentId) {
        const agentRecord = await getAgent(chat.agentId)
        if (!agentRecord) {
            error(404, 'Chat agent not found')
        }
        if (agentRecord.agentType === 'org' && locals.user.role !== 'admin') {
            error(403, 'Admin access required')
        }
        if (agentRecord.agentType === 'user' && agentRecord.userId !== locals.user.id) {
            error(403, 'Forbidden')
        }
        agent = { id: agentRecord.id, name: agentRecord.name, agentType: agentRecord.agentType }
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
    const activePathMessages = await chatMessageRepository.getActivePath(chat.id)
    const activePathToolCallIds = collectActiveUnansweredToolCallIds(activePathMessages)
    const allPendingApprovals = await toolApprovalRepository.getPendingForChatAll(
        chat.id,
        'approval',
    )
    const pendingApprovals = allPendingApprovals.filter(
        (approval) =>
            approval.toolCallId !== null && activePathToolCallIds.has(approval.toolCallId),
    )
    const resumableOAuth = await toolApprovalRepository.getForChatAll(
        chat.id,
        ['pending', 'approved'],
        'oauth',
    )
    const activeOAuth = resumableOAuth.filter(
        (approval) =>
            approval.toolCallId !== null && activePathToolCallIds.has(approval.toolCallId),
    )
    const pendingOAuth = activeOAuth.find((approval) => approval.status === 'pending') ?? null
    const approvedOAuth = activeOAuth.find((approval) => approval.status === 'approved') ?? null

    return {
        user: locals.user!,
        chat,
        messages,
        modelDisplayName,
        agent,
        uploadFilenames,
        pendingApprovals,
        pendingOAuth,
        approvedOAuth,
    }
}
