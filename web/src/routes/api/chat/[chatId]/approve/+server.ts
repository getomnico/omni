import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'
import { toolApprovalRepository } from '$lib/server/db/tool-approvals.js'

export const POST: RequestHandler = async ({ params, locals, request }) => {
    const logger = locals.logger.child('chat-approve')

    const chatId = params.chatId
    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    const chat = await chatRepository.get(chatId)
    if (!chat) {
        return json({ error: 'Chat not found' }, { status: 404 })
    }

    // Validate user owns the chat
    if (chat.userId !== locals.user.id) {
        return json({ error: 'Forbidden' }, { status: 403 })
    }

    try {
        const body = await request.json()
        const { approvalId, approvalIds, decision } = body as {
            approvalId?: string
            approvalIds?: string[]
            decision: 'approved' | 'denied'
        }
        const ids = approvalIds ?? (approvalId ? [approvalId] : [])

        if (ids.length === 0 || !decision) {
            return json(
                { error: 'approvalId/approvalIds and decision are required' },
                { status: 400 },
            )
        }

        if (decision !== 'approved' && decision !== 'denied') {
            return json({ error: 'decision must be "approved" or "denied"' }, { status: 400 })
        }

        const approvals = []
        for (const id of ids) {
            const approval = await toolApprovalRepository.get(id)
            if (!approval) {
                return json({ error: `Approval ${id} not found` }, { status: 404 })
            }
            if (approval.chatId !== chatId || approval.userId !== locals.user.id) {
                return json({ error: 'Forbidden' }, { status: 403 })
            }
            approvals.push(approval)
        }

        const resolvedApprovals = await toolApprovalRepository.resolveMany(
            ids,
            decision,
            locals.user.id,
        )

        logger.info('Tool approval resolved', {
            chatId,
            approvalIds: ids,
            decision,
            toolNames: approvals.map((approval) => approval.toolName),
        })

        return json({
            status: decision,
            approvalId: ids[0],
            approvalIds: resolvedApprovals.map((approval) => approval.id),
        })
    } catch (error) {
        logger.error('Error processing tool approval', error, { chatId })
        return json(
            {
                error: 'Failed to process approval',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
