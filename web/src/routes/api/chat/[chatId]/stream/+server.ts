import { readFile } from 'node:fs/promises'
import { relative, resolve } from 'node:path'
import { json, error } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { RequestHandler } from './$types.js'
import { chatMessageRepository, chatRepository } from '$lib/server/db/chats.js'
import { getAgent } from '$lib/server/db/agents.js'
import { isProviderConfigured } from '$lib/server/oauth/connectorOAuth.js'
import { getSourceDisplayName } from '$lib/utils/icons.js'
import { SourceType } from '$lib/types.js'
import type { OAuthRequiredAIEvent } from '$lib/types/message.js'

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

type TitleGenerationResponse = {
    status?: string
    title?: unknown
    reason?: unknown
}

const replayFixtureCookieName = 'omni-chat-stream-replay-fixture'

function replayStreamFixturePath(cookies: {
    get(name: string): string | undefined
}): string | null {
    const fixtureName = cookies.get(replayFixtureCookieName)?.trim()
    const fixtureDir = env.OMNI_TEST_CHAT_STREAM_REPLAY_FIXTURE_DIR?.trim()
    if (fixtureName && fixtureDir) {
        const baseDir = resolve(process.cwd(), fixtureDir)
        const fixturePath = resolve(baseDir, fixtureName)
        const relativePath = relative(baseDir, fixturePath)
        if (relativePath.startsWith('..') || resolve(relativePath) === relativePath) {
            throw error(400, 'Invalid replay fixture path')
        }
        return fixturePath
    }

    const fixturePath = env.OMNI_TEST_CHAT_STREAM_REPLAY_PATH?.trim()
    return fixturePath ? resolve(process.cwd(), fixturePath) : null
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
}

function eventData(event: string): string | null {
    for (const line of event.split('\n')) {
        if (line.startsWith('data:')) return line.substring(5).trim()
    }
    return null
}

// Test-only SSE replay shim. Production streams come from omni-ai below; this
// path is active only when OMNI_TEST_CHAT_STREAM_REPLAY_* is configured by the
// Playwright web server or explicitly in a test environment.
function replayStreamResponse(sampleStream: string, chatId: string): Response {
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
                let parentMessageId = (
                    await chatMessageRepository.getLastMessageInActivePath(chatId)
                )?.id

                for (const event of events) {
                    if (cancelled) return
                    let eventToSend = event
                    if (event.startsWith('event: message_id')) {
                        eventToSend = `event: message_id\ndata: sample-${runId}-${messageIdCounter++}`
                    } else if (event.startsWith('event: save_message')) {
                        // Test replay bypasses omni-ai, so emulate omni-ai's
                        // production save_message transform: persist the row in
                        // Postgres and send the browser the persisted message_id.
                        const data = eventData(event)
                        if (!data) continue
                        const savedMessage = await chatMessageRepository.create(
                            chatId,
                            JSON.parse(data),
                            parentMessageId,
                        )
                        parentMessageId = savedMessage.id
                        eventToSend = `event: message_id\ndata: ${savedMessage.id}`
                    }
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
            const result = (await response.json()) as TitleGenerationResponse
            logger.info('Title generation completed', {
                chatId,
                title: result.title,
                status: result.status,
                reason: result.reason,
            })
            if (result.status === 'skipped') {
                return { status: 'skipped' }
            }
            if (typeof result.title === 'string') {
                return { status: 'generated', title: result.title }
            }
            return { status: 'failed', message: 'Title generation returned an invalid response' }
        } else {
            const message = await aiErrorMessage(response)
            logger.warn('Title generation failed', {
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

export const GET: RequestHandler = async ({ params, locals, cookies, request, url }) => {
    const replayPath = replayStreamFixturePath(cookies)
    if (replayPath) {
        const sampleStream = await readFile(replayPath, 'utf-8')
        return replayStreamResponse(sampleStream, params.chatId)
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
        // Resume offset: native EventSource reconnects send the Last-Event-ID
        // header; our manual reconnects pass ?last_event_id=. Forward either to
        // the AI service so it can resume the buffered run from that point.
        const lastEventId =
            request.headers.get('last-event-id') ?? url.searchParams.get('last_event_id')
        let streamUrl = chat.agentId
            ? `${env.AI_SERVICE_URL}/chat/${chatId}/stream?auto_start=true`
            : `${env.AI_SERVICE_URL}/chat/${chatId}/stream`
        if (lastEventId) {
            streamUrl += `${streamUrl.includes('?') ? '&' : '?'}last_event_id=${encodeURIComponent(lastEventId)}`
        }
        const response = await fetch(streamUrl, {
            signal: abortController.signal,
            headers: lastEventId ? { 'Last-Event-ID': lastEventId } : undefined,
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
                                    logger.warn('Title generation failed', {
                                        chatId,
                                        message: result.message,
                                    })
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
                            let id = '' // Redis stream offset for Last-Event-ID resume

                            for (const line of lines) {
                                if (line.startsWith('event:')) {
                                    eventType = line.substring(6).trim()
                                } else if (line.startsWith('data:')) {
                                    data = line.substring(5).trim()
                                } else if (line.startsWith('id:')) {
                                    id = line.substring(3).trim()
                                }
                            }
                            // Preserve the offset on every reconstructed event below
                            // so the browser advances Last-Event-ID correctly.
                            const idPrefix = id ? `id: ${id}\n` : ''

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
                                const approvalEvent = `${idPrefix}event: approval_required\ndata: ${data}\n\n`
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
                                    const enrichedEvent = `${idPrefix}event: oauth_required\ndata: ${JSON.stringify(enriched)}\n\n`
                                    controller.enqueue(encoder.encode(enrichedEvent))
                                } catch (err) {
                                    logger.error('Failed to enrich oauth_required event', err, {
                                        chatId,
                                    })
                                    // Fall back to forwarding the raw event so the
                                    // client at least sees something actionable.
                                    const fallback = `${idPrefix}event: oauth_required\ndata: ${data}\n\n`
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

                                        const redactedEvent = `${idPrefix}event: message\ndata: ${JSON.stringify(redactedData)}\n\n`
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
                // The browser disconnected (e.g. backgrounded tab). Stop proxying
                // by aborting our upstream read, but do NOT touch the run — it
                // continues server-side so the client can reconnect and resume.
                // Generation is ended only via the explicit Stop endpoint.
                logger.info('Client disconnected from stream proxy', { chatId })
                reader.cancel()
                abortController.abort()
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
