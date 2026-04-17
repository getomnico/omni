import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { ChatRepository } from '$lib/server/db/chats.js'
import { ToolApprovalRepository } from '$lib/server/db/tool-approvals.js'

export const POST: RequestHandler = async ({ params, locals, request }) => {
    const logger = locals.logger.child('chat-approve')

    const chatId = params.chatId
    if (!chatId) {
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    const chatRepo = new ChatRepository(locals.db)
    const chat = await chatRepo.get(chatId)
    if (!chat) {
        return json({ error: 'Chat not found' }, { status: 404 })
    }

    try {
        const body = await request.json()
        const { approvalId, decision } = body as {
            approvalId: string
            decision: 'approved' | 'denied'
        }

        if (!approvalId || !decision) {
            return json({ error: 'approvalId and decision are required' }, { status: 400 })
        }

        if (decision !== 'approved' && decision !== 'denied') {
            return json({ error: 'decision must be "approved" or "denied"' }, { status: 400 })
        }

        // Update the approval record in the database
        const approvalRepo = new ToolApprovalRepository(locals.db)
        const approval = await approvalRepo.resolve(approvalId, decision, locals.user.id)
        if (!approval) {
            return json({ error: 'Approval not found' }, { status: 404 })
        }

        logger.info('Tool approval resolved', {
            chatId,
            approvalId,
            decision,
            toolName: approval.toolName,
        })

        return json({
            status: decision,
            approvalId,
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
