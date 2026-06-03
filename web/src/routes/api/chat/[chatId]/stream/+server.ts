import { readFile } from 'node:fs/promises'
import { relative, resolve } from 'node:path'
import { json, error } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'
import { chatRepository, chatMessageRepository } from '$lib/server/db/chats.js'
import { getAgent } from '$lib/server/db/agents.js'
import { isProviderConfigured } from '$lib/server/oauth/connectorOAuth.js'
import { getSourceDisplayName } from '$lib/utils/icons.js'
import { SourceType } from '$lib/types.js'
import type { OAuthRequiredAIEvent } from '$lib/types/message.js'
import type {
    ContentBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
} from '@anthropic-ai/sdk/resources/messages'

function sseEvent(eventType: string, data: object): string {
    return `event: ${eventType}\ndata: ${JSON.stringify(data)}\n\n`
}

function sseErrorResponse(message: string): Response {
    return new Response(sseEvent('stream_error', { message }), {
        status: 200,
        headers: {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache',
            Connection: 'keep-alive',
        },
    })
}

async function aiErrorMessage(response: Response): Promise<string> {
    try {
        const body: unknown = await response.json()
        if (body && typeof body === 'object' && 'detail' in body) {
            const detail = body.detail
            if (typeof detail === 'string') return detail
        }
        if (body && typeof body === 'object' && 'error' in body) {
            const error = body.error
            if (typeof error === 'string') return error
        }
    } catch {
        return `AI service unavailable (status ${response.status})`
    }

    return `AI service unavailable (status ${response.status})`
}

type TitleGenerationResult =
    | { status: 'generated'; title: string }
    | { status: 'skipped' }
    | { status: 'failed'; message: string }

const replayFixtureCookieName = 'omni-chat-stream-replay-fixture'

function replayStreamFixturePath(cookies: {
    get(name: string): string | undefined
}): string | null {
    const fixtureName = cookies.get(replayFixtureCookieName)?.trim()
    const fixtureDir = env.OMNI_CHAT_STREAM_REPLAY_FIXTURE_DIR?.trim()
    if (fixtureName && fixtureDir) {
        const baseDir = resolve(process.cwd(), fixtureDir)
        const fixturePath = resolve(baseDir, fixtureName)
        const relativePath = relative(baseDir, fixturePath)
        if (relativePath.startsWith('..') || resolve(relativePath) === relativePath) {
            throw error(400, 'Invalid replay fixture path')
        }
        return fixturePath
    }

    const fixturePath = env.OMNI_CHAT_STREAM_REPLAY_PATH?.trim()
    return fixturePath ? resolve(process.cwd(), fixturePath) : null
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

function replayStreamResponse(sampleStream: string): Response {
    const encoder = new TextEncoder()
    const runId = `${Date.now()}-${Math.random().toString(36).slice(2)}`
    let messageIdCounter = 0
    let cancelled = false

    const stream = new ReadableStream({
        async start(controller) {
            const events = sampleStream
                .split(/\n\n+/)
                .map((event) => event.trimEnd())
                .filter((event) => event.length > 0)

            try {
                for (const event of events) {
                    if (cancelled) return
                    const eventToSend = event.startsWith('event: message_id')
                        ? `event: message_id\ndata: sample-${runId}-${messageIdCounter++}`
                        : event
                    controller.enqueue(encoder.encode(`${eventToSend}\n\n`))
                    await sleep(10)
                }

                if (!sampleStream.includes('event: end_of_stream')) {
                    controller.enqueue(
                        encoder.encode('event: end_of_stream\ndata: Stream ended\n\n'),
                    )
                }
            } finally {
                if (!cancelled) controller.close()
            }
        },
        cancel() {
            cancelled = true
        },
    })

    return new Response(stream, {
        status: 200,
        headers: {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache',
            Connection: 'keep-alive',
        },
    })
}

async function triggerTitleGeneration(chatId: string, logger: any): Promise<TitleGenerationResult> {
    try {
        // First check if title already exists
        const chat = await chatRepository.get(chatId)
        if (chat?.title) {
            logger.debug('Chat already has a title, skipping title generation', { chatId })
            return { status: 'skipped' }
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
            return { status: 'generated', title: result.title }
        } else {
            const message = await aiErrorMessage(response)
            logger.warn('Title generation failed', undefined, {
                chatId,
                status: response.status,
                message,
            })
            return { status: 'failed', message }
        }
    } catch (error) {
        logger.warn('Error during title generation', error, { chatId })
        const message = error instanceof Error ? error.message : 'Failed to generate chat title'
        return { status: 'failed', message }
    }
}

export const GET: RequestHandler = async ({ params, locals, cookies }) => {
    const replayPath = replayStreamFixturePath(cookies)
    if (replayPath) {
        const sampleStream = await readFile(replayPath, 'utf-8')
        return replayStreamResponse(sampleStream)
    }

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
            return sseErrorResponse(await aiErrorMessage(response))
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
                            .then((result) => {
                                if (result.status === 'generated') {
                                    logger.info(
                                        `Generated title for chat ${chatId}: ${result.title}`,
                                    )
                                    controller.enqueue(
                                        encoder.encode(sseEvent('title', { title: result.title })),
                                    )
                                } else if (result.status === 'failed') {
                                    controller.enqueue(
                                        encoder.encode(
                                            sseEvent('title_error', { message: result.message }),
                                        ),
                                    )
                                }
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

                            // Enrich and forward oauth_required events to client.
                            // The AI service knows source_id/source_type/provider/url
                            // but only the web layer can answer "is the OAuth client
                            // configured for this provider" and resolve a friendly
                            // source display name — so we attach those fields here.
                            if (eventType === 'oauth_required' && data) {
                                try {
                                    const oauthData = JSON.parse(data) as OAuthRequiredAIEvent
                                    const providerConfigured = await isProviderConfigured(
                                        oauthData.provider,
                                    )
                                    const sourceDisplayName =
                                        getSourceDisplayName(oauthData.source_type as SourceType) ??
                                        oauthData.source_type
                                    const enriched = {
                                        ...oauthData,
                                        provider_configured: providerConfigured,
                                        source_display_name: sourceDisplayName,
                                    }
                                    const enrichedEvent = `event: oauth_required\ndata: ${JSON.stringify(enriched)}\n\n`
                                    controller.enqueue(encoder.encode(enrichedEvent))
                                } catch (err) {
                                    logger.error('Failed to enrich oauth_required event', err, {
                                        chatId,
                                    })
                                    // Fall back to forwarding the raw event so the
                                    // client at least sees something actionable.
                                    const fallback = `event: oauth_required\ndata: ${data}\n\n`
                                    controller.enqueue(encoder.encode(fallback))
                                }
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
                                                    source_type: searchResult.source_type ?? null,
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
                    const message =
                        error instanceof Error ? error.message : 'Failed to process chat stream'
                    controller.enqueue(encoder.encode(sseEvent('stream_error', { message })))
                    controller.close()
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
        const message = error instanceof Error ? error.message : 'Failed to process request'
        return sseErrorResponse(message)
    }
}
