import { json, error } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'
import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getAgent } from '$lib/server/db/agents.js'
import type {
    ContentBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
} from '@anthropic-ai/sdk/resources/messages'

async function triggerTitleGeneration(chatId: string, logger: any): Promise<string | null> {
    try {
        // First check if title already exists
        const chat = await chatRepository.get(chatId)
        if (chat?.title) {
            logger.debug('Chat already has a title, skipping title generation', { chatId })
            return null
        }

        logger.info('Triggering title generation', { chatId })

        const response = await fetch(`${env.AI_SERVICE_URL}/chat/${chatId}/generate_title`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
        })

        if (response.ok) {
            const result = await response.json()
            logger.info('Title generation completed', {
                chatId,
                title: result.title,
                status: result.status,
            })
            return result.title
        } else {
            logger.warn('Title generation failed', undefined, { chatId, status: response.status })
            return null
        }
    } catch (error) {
        logger.warn('Error during title generation', error, { chatId })
        return null
    }
}

export const GET: RequestHandler = async ({ params, locals }) => {
    const logger = locals.logger.child('chat')

    const chatId = params.chatId
    if (!chatId) {
        logger.warn('Missing chatId parameter in stream request')
        return json({ error: 'chatId parameter is required' }, { status: 400 })
    }

    const chat = await chatRepository.get(chatId)
    if (!chat) {
        logger.error('Chat not found', undefined, { chatId })
        return json({ error: 'Chat not found' }, { status: 404 })
    }

    // Agent chats require admin access
    if (chat.agentId) {
        const agent = await getAgent(chat.agentId)
        if (agent?.agentType === 'org' && locals.user?.role !== 'admin') {
            throw error(403, 'Admin access required for agent chats')
        }
    }

    logger.debug('Sending GET request to AI service to receive the streaming response', { chatId })

    const abortController = new AbortController()

    try {
        const streamUrl = chat.agentId
            ? `${env.AI_SERVICE_URL}/chat/${chatId}/stream?auto_start=true`
            : `${env.AI_SERVICE_URL}/chat/${chatId}/stream`
        const response = await fetch(streamUrl, {
            signal: abortController.signal,
        })

        if (!response.ok) {
            logger.error('AI service error', undefined, {
                status: response.status,
                statusText: response.statusText,
                chatId,
            })
            return json(
                {
                    error: 'AI service unavailable',
                    details: `Status: ${response.status}`,
                },
                { status: 502 },
            )
        }

        logger.info('Chat stream started successfully', { chatId })

        // Create a transformed stream that:
        // 1. Intercepts save_message events for database writes
        // 2. Filters out save_message events from client
        // 3. Triggers title generation after completion
        const reader = response.body?.getReader()

        if (!reader) {
            throw new Error('Response body is null')
        }

        // Track the last saved message ID to chain parent_id during streaming
        const lastActiveMessage = await chatMessageRepository.getLastMessageInActivePath(chatId)
        let lastSavedMessageId: string | undefined = lastActiveMessage?.id

        const decoder = new TextDecoder()
        const encoder = new TextEncoder()
        let buffer = ''

        const stream = new ReadableStream({
            async start(controller) {
                try {
                    if (!chat.title) {
                        logger.info('Generating title for chat', { chatId })
                        triggerTitleGeneration(chatId, logger)
                            .then((title) => {
                                logger.info(`Generated title for chat ${chatId}: ${title}`)
                                const event = `event: title\ndata: ${title}\n\n`
                                controller.enqueue(encoder.encode(event))
                            })
                            .catch((err) =>
                                logger.error(`Failed to generate title for chat ${chatId}`, err),
                            )
                    }

                    while (true) {
                        const { done, value } = await reader.read()

                        if (done) {
                            controller.close()
                            break
                        }

                        // Decode chunk and add to buffer
                        buffer += decoder.decode(value, { stream: true })

                        // Process complete SSE events in buffer
                        const events = buffer.split('\n\n')
                        // Keep the last incomplete event in buffer
                        buffer = events.pop() || ''

                        for (const event of events) {
                            if (!event.trim()) continue

                            const lines = event.split('\n')
                            let eventType = 'message' // default event type
                            let data = ''

                            for (const line of lines) {
                                if (line.startsWith('event:')) {
                                    eventType = line.substring(6).trim()
                                } else if (line.startsWith('data:')) {
                                    data = line.substring(5).trim()
                                }
                            }

                            // If this is a save_message event, save it immediately to database
                            if (eventType === 'save_message' && data) {
                                try {
                                    const message = JSON.parse(data)
                                    const { id: messageId } = await chatMessageRepository.create(
                                        chatId,
                                        message,
                                        lastSavedMessageId,
                                    )
                                    lastSavedMessageId = messageId
                                    logger.debug('Saved message to database', {
                                        chatId,
                                        role: message.role,
                                        messageId,
                                    })

                                    // Send message ID to client
                                    const event = `event: message_id\ndata: ${messageId}\n\n`
                                    controller.enqueue(encoder.encode(event))
                                } catch (error) {
                                    logger.error('Failed to save message to database', error, {
                                        chatId,
                                        data,
                                    })
                                }
                                // Don't forward save_message events to client (internal only)
                                continue
                            }

                            // Forward approval_required events to client
                            if (eventType === 'approval_required' && data) {
                                try {
                                    const approvalData = JSON.parse(data)
                                    // Save approval record to database using the same ID from Redis
                                    const { toolApprovalRepository } =
                                        await import('$lib/server/db/tool-approvals.js')
                                    await toolApprovalRepository.createWithId(
                                        approvalData.approval_id,
                                        chatId,
                                        chat.userId,
                                        approvalData.tool_name,
                                        approvalData.tool_input,
                                    )
                                } catch (err) {
                                    logger.error('Failed to save tool approval record', err, {
                                        chatId,
                                    })
                                }
                                const approvalEvent = `event: approval_required\ndata: ${data}\n\n`
                                controller.enqueue(encoder.encode(approvalEvent))
                                continue
                            }

                            // Special handling for certain events
                            if (eventType === 'message' && data) {
                                try {
                                    const parsedData = JSON.parse(data)
                                    // Redact tool result content before forwarding to client
                                    // Check if this is a tool_result block
                                    if (
                                        parsedData.type === 'tool_result' &&
                                        Array.isArray(parsedData.content)
                                    ) {
                                        // Separate search results from other content types
                                        const hasSearchResults = parsedData.content.some(
                                            (item: any) => item.type === 'search_result',
                                        )

                                        let redactedContent
                                        if (hasSearchResults) {
                                            // Redact search result content (keep title/source, remove highlights)
                                            redactedContent = parsedData.content
                                                .filter(
                                                    (item: any) => item.type === 'search_result',
                                                )
                                                .map((searchResult: any) => ({
                                                    type: 'search_result',
                                                    title: searchResult.title,
                                                    source: searchResult.source,
                                                    content: [], // Redact the highlights
                                                }))
                                        } else {
                                            // For non-search results (connector actions, sandbox, etc.)
                                            // Forward text content as-is (truncated for safety)
                                            redactedContent = parsedData.content.map(
                                                (item: any) => {
                                                    if (
                                                        item.type === 'text' &&
                                                        item.text?.length > 5000
                                                    ) {
                                                        return {
                                                            ...item,
                                                            text:
                                                                item.text.substring(0, 5000) +
                                                                '\n... (truncated)',
                                                        }
                                                    }
                                                    return item
                                                },
                                            )
                                        }

                                        const redactedData = {
                                            ...parsedData,
                                            content: redactedContent,
                                        }

                                        const redactedEvent = `event: message\ndata: ${JSON.stringify(redactedData)}\n\n`
                                        controller.enqueue(encoder.encode(redactedEvent))
                                        continue
                                    }
                                } catch (parseError) {
                                    // If parsing fails, forward as-is
                                }
                            }

                            // Forward all other events to the client as-is
                            const eventStr = event + '\n\n'
                            controller.enqueue(encoder.encode(eventStr))
                        }
                    }
                } catch (error) {
                    logger.error('Error in stream processing', error, { chatId })
                    controller.error(error)
                }
            },
            async cancel() {
                logger.info('Client disconnected, cancelling stream', { chatId })
                reader.cancel()
                abortController.abort()

                // Clean up any incomplete tool uses from the interrupted stream
                try {
                    const lastMessage =
                        await chatMessageRepository.getLastMessageInActivePath(chatId)
                    if (!lastMessage || lastMessage.message.role !== 'assistant') return

                    const content = lastMessage.message.content
                    const contentBlocks: ContentBlockParam[] = Array.isArray(content) ? content : []
                    const toolUseBlocks = contentBlocks.filter(
                        (b): b is ToolUseBlockParam => b.type === 'tool_use',
                    )
                    if (toolUseBlocks.length === 0) return

                    const allMessages = await chatMessageRepository.getByChatId(chatId)
                    const toolResultMsgs = allMessages.filter((m) => m.parentId === lastMessage.id)
                    const existingToolUseIds = new Set<string>()
                    for (const msg of toolResultMsgs) {
                        if (msg.message.role === 'user' && Array.isArray(msg.message.content)) {
                            for (const block of msg.message.content) {
                                if (block.type === 'tool_result') {
                                    existingToolUseIds.add(block.tool_use_id)
                                }
                            }
                        }
                    }

                    const missingToolUses = toolUseBlocks.filter(
                        (tu) => !existingToolUseIds.has(tu.id),
                    )
                    if (missingToolUses.length === 0) return

                    const syntheticToolResults: ToolResultBlockParam[] = missingToolUses.map(
                        (tu) => ({
                            type: 'tool_result',
                            tool_use_id: tu.id,
                            content: [{ type: 'text', text: 'Tool response was interrupted' }],
                            is_error: true,
                        }),
                    )

                    await chatMessageRepository.create(
                        chatId,
                        { role: 'user', content: syntheticToolResults },
                        lastMessage.id,
                    )

                    logger.info('Cleaned up interrupted stream', {
                        chatId,
                        missingToolCount: missingToolUses.length,
                    })
                } catch (err) {
                    logger.error('Error cleaning up interrupted stream', err, { chatId })
                }
            },
        })

        // Return the streaming response with SSE headers
        return new Response(stream, {
            status: 200,
            headers: {
                'Content-Type': 'text/event-stream',
                'Cache-Control': 'no-cache',
                Connection: 'keep-alive',
            },
        })
    } catch (error) {
        logger.error('Error calling AI service', error, { chatId })
        return json(
            {
                error: 'Failed to process request',
                details: error instanceof Error ? error.message : 'Unknown error',
            },
            { status: 500 },
        )
    }
}
