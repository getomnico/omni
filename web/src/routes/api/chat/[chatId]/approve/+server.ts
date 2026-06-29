import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import { chatMessageRepository, chatRepository } from '$lib/server/db/chats.js'
import type {
    MessageParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
} from '@anthropic-ai/sdk/resources/messages.js'
import { toolApprovalRepository } from '$lib/server/db/tool-approvals.js'

function isToolUseBlock(block: unknown): block is ToolUseBlockParam {
    return (
        typeof block === 'object' &&
        block !== null &&
        'type' in block &&
        block.type === 'tool_use' &&
        'id' in block &&
        typeof block.id === 'string'
    )
}

type ActiveToolCallContext = {
    toolCallIds: Set<string>
    parentMessageIdsByToolCallId: Map<string, string>
}

async function activeToolCallContext(chatId: string): Promise<ActiveToolCallContext> {
    const messages = await chatMessageRepository.getActivePath(chatId)
    const toolCallIds = new Set<string>()
    const parentMessageIdsByToolCallId = new Map<string, string>()
    for (const message of messages) {
        const content = message.message.content
        if (!Array.isArray(content)) continue
        for (const block of content) {
            if (isToolUseBlock(block)) {
                toolCallIds.add(block.id)
                parentMessageIdsByToolCallId.set(block.id, message.id)
            }
        }
    }
    return { toolCallIds, parentMessageIdsByToolCallId }
}

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

        const { toolCallIds, parentMessageIdsByToolCallId } = await activeToolCallContext(chatId)
        const staleApproval = approvals.find(
            (approval) => approval.toolCallId === null || !toolCallIds.has(approval.toolCallId),
        )
        if (staleApproval) {
            return json(
                { error: 'Approval is no longer on the active chat branch' },
                { status: 409 },
            )
        }

        let denialMessageId: string | null = null
        if (decision === 'denied') {
            const parentMessageIds = new Set(
                approvals.map((approval) =>
                    approval.toolCallId
                        ? parentMessageIdsByToolCallId.get(approval.toolCallId)
                        : undefined,
                ),
            )
            parentMessageIds.delete(undefined)
            if (parentMessageIds.size !== 1) {
                return json(
                    { error: 'Cannot deny approvals from multiple assistant turns at once' },
                    { status: 409 },
                )
            }

            const denialResults: ToolResultBlockParam[] = approvals.map((approval) => ({
                type: 'tool_result',
                tool_use_id: approval.toolCallId!,
                content: [
                    {
                        type: 'text',
                        text: 'The user denied approval for this tool call.',
                    },
                ],
                is_error: true,
            }))
            const denialMessage: MessageParam = {
                role: 'user',
                content: denialResults,
            }
            const savedDenialMessage = await chatMessageRepository.create(
                chatId,
                denialMessage,
                [...parentMessageIds][0],
            )
            denialMessageId = savedDenialMessage.id
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
            denialMessageId,
        })

        return json({
            status: decision,
            approvalId: ids[0],
            approvalIds: resolvedApprovals.map((approval) => approval.id),
            denialMessageId,
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
