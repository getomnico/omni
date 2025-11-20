<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import type {
        MessageParam,
        MessageStreamEvent,
        SearchResultBlockParam,
        TextBlockParam,
        TextCitationParam,
        ToolUseBlock,
        TextDelta,
        CitationsDelta,
        InputJSONDelta,
    } from '@anthropic-ai/sdk/resources/messages'
    import {
        Copy,
        ThumbsUp,
        ThumbsDown,
        Share,
        Check,
        CircleAlert,
        CircleAlertIcon,
        ExternalLink,
        FileText,
    } from '@lucide/svelte'
    import { marked } from 'marked'
    import { onMount } from 'svelte'
    import type { PageProps } from './$types'
    import type {
        ProcessedMessage,
        TextMessageContent,
        ToolMessageContent,
        MessageContent,
    } from '$lib/types/message'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import { cn } from '$lib/utils'
    import type { ToolResultBlockParam } from '@anthropic-ai/sdk/resources'
    import { page } from '$app/state'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import { type ChatMessage } from '$lib/server/db/schema'
    import type { ContentBlockParam } from '@anthropic-ai/sdk/resources.js'
    import { afterNavigate, invalidate } from '$app/navigation'
    import UserInput from '$lib/components/user-input.svelte'
    import * as Alert from '$lib/components/ui/alert'
    import type { Attachment } from 'svelte/attachments'
    import * as HoverCard from '$lib/components/ui/hover-card'
    import {
        getIconFromSearchResult,
        getSourceDisplayName,
        inferSourceFromUrl,
    } from '$lib/utils/icons'
    import { SourceType } from '$lib/types'
    import MarkdownMessage from '$lib/components/markdown-message.svelte'

    let { data }: PageProps = $props()
    let chatMessages = $state<ChatMessage[]>([...data.messages])

    afterNavigate(() => {
        chatMessages = [...data.messages]
    })

    let userMessage = $state('')
    let chatContainerRef: HTMLDivElement
    let lastUserMessageRef: HTMLDivElement | null = $state(null)

    let isStreaming = $state(false)
    let error = $state<string | null>(null)

    let processedMessages = $derived(processMessages(chatMessages))
    $inspect(processedMessages).with((t, v) => console.log('processedMessages', t, v))
    $inspect(chatMessages).with((t, v) => console.log('chatMessages', t, v))
    let copiedMessageId = $state<number | null>(null)
    let copiedUrl = $state(false)
    let messageFeedback = $state<Record<string, 'upvote' | 'downvote'>>({})

    function copyMessageToClipboard(message: ProcessedMessage) {
        const content = message.content
            .map((block) => {
                if (block.type === 'text') {
                    return (block as TextMessageContent).text
                } else if (block.type === 'tool') {
                    const toolBlock = block as ToolMessageContent

                    if (toolBlock.toolResult?.content && toolBlock.toolResult.content.length > 0) {
                        let toolText = 'Sources:'
                        toolBlock.toolResult.content.forEach((result) => {
                            toolText += `\n• ${result.title} - ${result.source}`
                        })
                        return toolText
                    }
                }
                return ''
            })
            .filter((text) => text.length > 0)
            .join('\n\n')

        navigator.clipboard.writeText(content)
        copiedMessageId = message.id
        setTimeout(() => {
            copiedMessageId = null
        }, 2000)
    }

    function copyCurrentUrlToClipboard() {
        navigator.clipboard.writeText(window.location.href)
        copiedUrl = true
        setTimeout(() => {
            copiedUrl = false
        }, 2000)
    }

    async function handleFeedback(messageId: string, feedbackType: 'upvote' | 'downvote') {
        try {
            await fetch(`/api/chat/${data.chat.id}/messages/${messageId}/feedback`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({ feedbackType }),
            })

            messageFeedback[messageId] = feedbackType
        } catch (error) {
            console.error('Failed to submit feedback:', error)
        }
    }

    // Assumption: only one thinking tag in the input
    // AWS Nova Pro returns content <thinking> tags that is just superfluous, so get rid of it
    function stripThinkingContent(messageStr: string, thinkingTagName: string): string {
        const startTagIdx = messageStr.indexOf(`<${thinkingTagName}>`)
        if (startTagIdx === -1) {
            return messageStr
        }

        const endTagIdx = messageStr.indexOf(`</${thinkingTagName}>`, startTagIdx)
        if (endTagIdx === -1) {
            return messageStr.slice(0, startTagIdx)
        }

        const res =
            messageStr.slice(0, startTagIdx) +
            messageStr.slice(endTagIdx + thinkingTagName.length + 3)
        return res
    }

    function collectSources(message: ProcessedMessage): TextCitationParam[] {
        const citations = []
        const sourceSet = new Set()
        for (const block of message.content) {
            if (block.type === 'text' && block.citations) {
                // TODO: Handle other types of citations if necessary
                for (const citation of block.citations) {
                    if (
                        citation.type === 'search_result_location' &&
                        !sourceSet.has(citation.source)
                    ) {
                        citations.push(citation)
                        sourceSet.add(citation.source)
                    }
                }
            }
        }
        return citations
    }

    function processMessages(chatMessages: ChatMessage[]): ProcessedMessage[] {
        const processedMessages: ProcessedMessage[] = []

        const addMessage = (message: ProcessedMessage) => {
            const lastMessage = processedMessages[processedMessages.length - 1]
            let messageToUpdate: ProcessedMessage
            if (!lastMessage || lastMessage.role !== message.role) {
                const newMessage = {
                    id: processedMessages.length,
                    origMessageId: message.origMessageId,
                    role: message.role,
                    content: [] as MessageContent,
                }
                processedMessages.push(newMessage)
                messageToUpdate = newMessage
            } else {
                messageToUpdate = lastMessage
            }

            for (const block of message.content) {
                const lastBlock = messageToUpdate.content[messageToUpdate.content.length - 1]
                if (lastBlock && lastBlock.type === 'text' && block.type === 'text') {
                    // Combine text blocks
                    lastBlock.text += block.text
                    if (block.citations) {
                        if (!lastBlock.citations) {
                            lastBlock.citations = []
                        }
                        lastBlock.citations.push(...block.citations)
                    }
                } else {
                    messageToUpdate.content.push({
                        ...block,
                        id: messageToUpdate.content.length,
                    })
                }
            }
        }

        const updateToolResults = (toolResult: ToolMessageContent['toolResult']) => {
            if (!toolResult) return
            for (const message of processedMessages) {
                if (message.role === 'assistant') {
                    for (const block of message.content) {
                        if (block.type === 'tool' && block.toolUse.id === toolResult.toolUseId) {
                            block.toolResult = toolResult
                            return
                        }
                    }
                }
            }
        }

        const messages = chatMessages.map((m) => m.message)
        for (let i = 0; i < messages.length; i++) {
            const message = messages[i]
            const messageCitations = [] // All citations in this message
            if (isUserMessage(message)) {
                // User messages are expected to contain only text blocks
                const userMessageContent: MessageContent =
                    typeof message.content === 'string'
                        ? [{ id: 0, type: 'text', text: message.content }]
                        : message.content
                              .filter((b) => b.type === 'text')
                              .map((b, bi) => ({
                                  id: bi,
                                  type: 'text',
                                  text: b.text,
                              }))

                const processedUserMessage: ProcessedMessage = {
                    id: processedMessages.length,
                    origMessageId: chatMessages[i].id,
                    role: 'user',
                    content: userMessageContent,
                }

                addMessage(processedUserMessage)
            } else {
                // Here we handle both assistant messages (with possible tool uses) and also user messages that contain tool results
                const processedMessage: ProcessedMessage = {
                    id: processedMessages.length,
                    origMessageId: chatMessages[i].id,
                    role: 'assistant',
                    content: [],
                }

                const contentBlocks = Array.isArray(message.content)
                    ? message.content
                    : [{ type: 'text', text: message.content } as TextBlockParam]

                for (let blockIdx = 0; blockIdx < contentBlocks.length; blockIdx++) {
                    const block = contentBlocks[blockIdx]
                    if (block.type === 'text') {
                        let citationTxt = ''
                        for (const citation of block.citations || []) {
                            const citationIdx = messageCitations.length
                            messageCitations.push(citation)
                            citationTxt += ` [${citationIdx}]`
                        }
                        processedMessage.content.push({
                            id: processedMessage.content.length,
                            type: 'text',
                            text: citationTxt ? `${block.text} ${citationTxt}` : block.text,
                            citations: block.citations ? [...block.citations] : undefined,
                        })
                    } else {
                        // Tool use or result
                        if (block.type === 'tool_use') {
                            // Tool use always comes first, so we create the corresponding output block
                            const toolMsgContent: ToolMessageContent = {
                                id: 0,
                                type: 'tool',
                                toolUse: {
                                    id: block.id,
                                    name: block.name,
                                    input: block.input,
                                },
                            }

                            processedMessage.content.push(toolMsgContent)
                        } else if (block.type === 'tool_result') {
                            const toolUseId = block.tool_use_id
                            const searchResults = Array.isArray(block.content)
                                ? (block.content.filter(
                                      (b) => b.type === 'search_result',
                                  ) as SearchResultBlockParam[])
                                : []
                            updateToolResults({
                                toolUseId,
                                content: searchResults.map((r) => ({
                                    title: r.title,
                                    source: r.source,
                                })),
                            })
                        }
                    }
                }

                // Add a separate block containing all the citation links
                // This will append a text block at the end that looks like the following:
                //      [Marked]: https://github.com/markedjs/marked/
                //      [Markdown]: http://daringfireball.net/projects/markdown/
                if (messageCitations.length > 0) {
                    const citationSourceTxt = messageCitations
                        .map((c, idx) => {
                            if (c.type === 'search_result_location') {
                                return `[${idx}]: ${c.source}`
                            }
                        })
                        .filter((t) => t !== undefined)
                        .join('\n')

                    processedMessage.content.push({
                        id: processedMessage.content.length,
                        type: 'text',
                        text: `\n\n${citationSourceTxt}\n\n`,
                    })
                }

                addMessage(processedMessage)
            }
        }

        return processedMessages
    }

    function isUserMessage(message: MessageParam) {
        // Tool results, even though found in messages with role 'user', are shown as assistant messages
        const toolResults = Array.isArray(message.content)
            ? message.content.some((block) => block.type === 'tool_result')
            : false
        return message.role === 'user' && !toolResults
    }

    function scrollToBottom() {
        requestAnimationFrame(() => {
            if (chatContainerRef) {
                chatContainerRef.scrollTo({
                    top: chatContainerRef.scrollHeight,
                    behavior: 'smooth',
                })
            }
        })
    }

    function scrollUserMessageToTop() {
        requestAnimationFrame(() => {
            if (lastUserMessageRef && chatContainerRef) {
                // Scroll so the user message appears at the top of the viewport
                const messageTop = lastUserMessageRef.offsetTop - chatContainerRef.offsetTop
                chatContainerRef.scrollTo({ top: messageTop, behavior: 'smooth' })
            }
        })
    }

    // This will trigger the streaming of AI response when the component is mounted
    // If no response is currently being streamed, nothing happens
    onMount(() => {
        if ((page.state as any).stream) {
            streamResponse(data.chat.id)
        }
    })

    function streamResponse(chatId: string) {
        isStreaming = true
        error = null

        let currToolUseId: string
        let currToolUseName: string
        let currToolUseInputStr: string

        const eventSource = new EventSource(`/api/chat/${chatId}/stream`, { withCredentials: true })

        let streamCompleted = false
        let messageEventsReceived = 0

        const collectStreamingResponse = (
            block:
                | ToolUseBlock
                | TextDelta
                | InputJSONDelta
                | ToolResultBlockParam
                | CitationsDelta,
            blockIdx?: number, // This should be defined for all block types above except ToolResultBlockParam (since this one doesn't come from the LLM)
        ) => {
            const lastMessage = chatMessages[chatMessages.length - 1]
            if (!lastMessage) {
                // This should never happen
                console.error('No last message found when streaming response')
                return
            }

            const existingBlocks = lastMessage.message.content as ContentBlockParam[]
            if (block.type === 'text_delta') {
                if (blockIdx === undefined) {
                    throw new Error('blockIdx is required for text_delta')
                }
                if (blockIdx >= existingBlocks.length) {
                    existingBlocks.push({
                        type: 'text',
                        text: block.text,
                    })
                } else {
                    const existingBlock = existingBlocks[blockIdx]
                    if (existingBlock.type !== 'text') {
                        throw new Error(
                            `Error handling text_delta, existing block at index ${blockIdx} is not a text block`,
                        )
                    }
                    existingBlock.text += block.text
                }
            } else if (block.type === 'citations_delta') {
                if (blockIdx === undefined) {
                    throw new Error('blockIdx is required for citations_delta')
                }
                if (blockIdx >= existingBlocks.length) {
                    existingBlocks.push({
                        type: 'text',
                        text: '',
                        citations: [block.citation],
                    })
                } else {
                    const existingBlock = existingBlocks[blockIdx]
                    if (existingBlock.type !== 'text') {
                        throw new Error(
                            `Error handling citations_delta, existing block at index ${blockIdx} is not a text block`,
                        )
                    }
                    if (!existingBlock.citations) {
                        existingBlock.citations = []
                    }
                    existingBlock.citations.push(block.citation)
                }
            } else if (block.type === 'tool_use') {
                if (blockIdx === undefined) {
                    throw new Error('blockIdx is required for tool_use block')
                }
                if (blockIdx >= existingBlocks.length) {
                    existingBlocks.push({
                        type: 'tool_use',
                        id: block.id,
                        name: block.name,
                        input: block.input,
                    })
                } else {
                    // We could also use blockIdx, but we use the id instead
                    const existingToolUse = existingBlocks.find(
                        (b) => b.type === 'tool_use' && b.id === block.id,
                    )

                    // TODO: Instead of updating the input JSON in one go, handle input_json_delta in this method instead
                    // Currently, the caller to this method is accumulating all the input JSON deltas and sending it in a
                    // single tool_use block
                    if (existingToolUse) {
                        ;(existingToolUse as ToolUseBlock).input = block.input
                    } else {
                        // TODO: This should never happen, because we add a new block above in the blockIdx check
                        existingBlocks.push({
                            type: 'tool_use',
                            id: block.id,
                            name: block.name,
                            input: block.input,
                        })
                    }
                }
            } else if (block.type === 'tool_result') {
                // Push a new message with the tool result
                const lastMessage = chatMessages[chatMessages.length - 1]
                if (lastMessage && lastMessage.message.role === 'user') {
                    const blocks = lastMessage.message.content
                    if (Array.isArray(blocks)) {
                        blocks.push(block)
                    }
                } else {
                    chatMessages.push({
                        id: `temp-${Date.now()}`,
                        chatId,
                        message: {
                            role: 'user',
                            content: [block],
                        },
                        messageSeqNum: chatMessages.length + 1,
                        createdAt: new Date(),
                    })
                }
            }
        }

        eventSource.addEventListener('message_id', (event) => {
            const messageId = event.data
            const lastMessage = chatMessages[chatMessages.length - 1]
            if (lastMessage && lastMessage.id.toString().startsWith('temp-')) {
                lastMessage.id = messageId
            }
        })

        eventSource.addEventListener('title', (event) => {
            invalidate('app:recent_chats') // This will force a re-fetch of recent chats and update the title in the sidebar
        })

        eventSource.addEventListener('message', (event) => {
            try {
                const data: MessageStreamEvent | ToolResultBlockParam = JSON.parse(event.data)
                if (data.type === 'message_start') {
                    chatMessages.push({
                        id: `temp-${Date.now()}`,
                        chatId,
                        message: {
                            role: data.message.role,
                            content: data.message.content,
                        },
                        messageSeqNum: chatMessages.length + 1,
                        createdAt: new Date(),
                    })
                } else if (data.type === 'content_block_start') {
                    if (
                        data.content_block.type === 'tool_use' &&
                        (data.content_block.name === 'search_documents' ||
                            data.content_block.name === 'read_document')
                    ) {
                        collectStreamingResponse(data.content_block, data.index)
                        currToolUseId = data.content_block.id
                        currToolUseName = data.content_block.name
                        currToolUseInputStr = ''
                    }
                } else if (data.type === 'content_block_delta') {
                    if (data.delta.type === 'text_delta' && data.delta.text) {
                        collectStreamingResponse(data.delta, data.index)
                    } else if (data.delta.type === 'citations_delta') {
                        collectStreamingResponse(data.delta, data.index)
                    } else if (data.delta.type === 'input_json_delta') {
                        // Parse partial JSON to show search query if possible
                        currToolUseInputStr += data.delta.partial_json
                        try {
                            const parsedInput = JSON.parse(currToolUseInputStr)
                            collectStreamingResponse(
                                {
                                    type: 'tool_use',
                                    id: currToolUseId,
                                    name: currToolUseName,
                                    input: parsedInput,
                                },
                                data.index,
                            )
                        } catch (err) {
                            // Ignore JSON parse errors for partial input
                        }
                    }
                } else if (data.type == 'tool_result') {
                    collectStreamingResponse(data)
                }

                scrollToBottom()
            } catch (err) {
                console.error('Failed to parse SSE data:', event.data, err)
            } finally {
                messageEventsReceived += 1
            }
        })

        eventSource.addEventListener('end_of_stream', () => {
            streamCompleted = true
            isStreaming = false
            eventSource.close()

            if (messageEventsReceived === 0 && !error) {
                error = 'Failed to generate response. Please try again.'
            }
        })

        eventSource.addEventListener('error', (event) => {
            error = 'Failed to generate response. Please try again.'
            isStreaming = false
            eventSource.close()
        })
    }

    async function handleSubmit() {
        const userMsg = userMessage.trim()
        if (userMsg) {
            const response = await fetch(`/api/chat/${data.chat.id}/messages`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: userMsg,
                    role: 'user',
                }),
            })

            if (!response.ok) {
                console.error('Failed to send message to chat session')
                return
            }

            const { messageId } = await response.json()
            console.log('Sent message with ID:', messageId)

            const newUserMessage: ChatMessage = {
                id: messageId,
                chatId: data.chat.id,
                message: {
                    role: 'user',
                    content: userMsg,
                },
                messageSeqNum: data.messages.length,
                createdAt: new Date(),
            }
            chatMessages.push(newUserMessage)

            userMessage = ''

            // Scroll to show the new user message at the top
            scrollUserMessageToTop()

            // Start streaming AI response
            streamResponse(data.chat.id)
        }
    }

    const attachInlineCitations: Attachment = (container: Element) => {
        const inlineCitations = container.querySelectorAll('.inline-citation')
        let lastChild
        for (const child of container.childNodes) {
            if (child instanceof HTMLElement && !child.classList.contains('inline-citation')) {
                lastChild = child
            }
        }

        if (lastChild) {
            // Add all citations to the last child
            for (const inlineCitation of inlineCitations) {
                container.removeChild(inlineCitation)
                lastChild.appendChild(inlineCitation)
            }
        }

        return () => {}
    }

    // Remove markdown annotations, reduce consecutive whitespace to a single space, truncate to 80 chars
    function sanitizeCitedText(text: string) {
        // Remove markdown formatting
        let sanitized = text
            // Remove bold/italic markers
            .replace(/\*\*([^*]+)\*\*/g, '$1') // **bold**
            .replace(/\*([^*]+)\*/g, '$1') // *italic*
            .replace(/__([^_]+)__/g, '$1') // __bold__
            .replace(/_([^_]+)_/g, '$1') // _italic_
            // Remove links [text](url)
            .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
            // Remove inline code
            .replace(/`([^`]+)`/g, '$1')
            // Remove headers
            .replace(/^#+\s+/gm, '')
            // Replace multiple ellipses with single ellipsis
            .replace(/\.{2,}/g, '... ')
            // Reduce consecutive whitespace to single space
            .replace(/\s+/g, ' ')
            // Trim
            .trim()

        // Truncate to 80 chars with ellipsis
        if (sanitized.length > 80) {
            sanitized = sanitized.substring(0, 80) + '...'
        }

        return sanitized
    }

    function extractDomain(url: string): string {
        try {
            const urlObj = new URL(url)
            return urlObj.hostname
        } catch {
            return ''
        }
    }
</script>

<svelte:head>
    <title>{data.chat.title} - Omni</title>
</svelte:head>

{#snippet messageControls(message: ProcessedMessage)}
    <div class="mt-2 flex items-center justify-start gap-0.5" data-role="message-controls">
        <!-- Copy message, feedback upvote/downvote -->
        <Tooltip.Provider delayDuration={300}>
            <Tooltip.Root>
                <Tooltip.Trigger>
                    <Button
                        class="cursor-pointer"
                        size="icon"
                        variant="ghost"
                        onclick={() => copyMessageToClipboard(message)}>
                        {#if copiedMessageId === message.id}
                            <Check class="h-4 w-4 text-green-600" />
                        {:else}
                            <Copy class="h-4 w-4" />
                        {/if}
                    </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>
                    <p>Copy message</p>
                </Tooltip.Content>
            </Tooltip.Root>
        </Tooltip.Provider>
        {#if !messageFeedback[message.origMessageId] || messageFeedback[message.origMessageId] === 'upvote'}
            <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                    <Tooltip.Trigger>
                        <Button
                            class={cn(
                                'cursor-pointer',
                                messageFeedback[message.origMessageId] === 'upvote' &&
                                    'text-green-600',
                            )}
                            size="icon"
                            variant="ghost"
                            onclick={() => handleFeedback(message.origMessageId, 'upvote')}>
                            <ThumbsUp class="h-4 w-4" />
                        </Button>
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                        <p>Good response</p>
                    </Tooltip.Content>
                </Tooltip.Root>
            </Tooltip.Provider>
        {/if}
        {#if !messageFeedback[message.origMessageId] || messageFeedback[message.origMessageId] === 'downvote'}
            <Tooltip.Provider delayDuration={300}>
                <Tooltip.Root>
                    <Tooltip.Trigger>
                        <Button
                            class={cn(
                                'cursor-pointer',
                                messageFeedback[message.origMessageId] === 'downvote' &&
                                    'text-red-600',
                            )}
                            size="icon"
                            variant="ghost"
                            onclick={() => handleFeedback(message.origMessageId, 'downvote')}>
                            <ThumbsDown class="h-4 w-4" />
                        </Button>
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                        <p>Bad response</p>
                    </Tooltip.Content>
                </Tooltip.Root>
            </Tooltip.Provider>
        {/if}
        <Tooltip.Provider delayDuration={300}>
            <Tooltip.Root>
                <Tooltip.Trigger>
                    <Button
                        class="cursor-pointer"
                        size="icon"
                        variant="ghost"
                        onclick={copyCurrentUrlToClipboard}>
                        {#if copiedUrl}
                            <Check class="h-4 w-4 text-green-600" />
                        {:else}
                            <Share class="h-4 w-4" />
                        {/if}
                    </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>
                    <p>Share</p>
                </Tooltip.Content>
            </Tooltip.Root>
        </Tooltip.Provider>
    </div>
{/snippet}

{#snippet inlineCitations(citations: TextCitationParam[])}
    {#if citations.length > 0}
        <span class="not-prose inline-citation ml-1 inline-flex items-start">
            {#each citations as citation}
                {#if citation.type === 'search_result_location'}
                    <HoverCard.Root>
                        <HoverCard.Trigger
                            href={citation.source}
                            target="_blank"
                            rel="noreferrer noopener"
                            class="border-primary/10 bg-muted text-muted-foreground hover:border-primary/20 hover:text-foreground/80 inline-block max-w-36 items-center 
                            gap-1 truncate overflow-hidden rounded-lg border px-1 py-0.5 text-xs no-underline 
                            transition-colors">
                            {extractDomain(citation.source)}
                        </HoverCard.Trigger>
                        <HoverCard.Content>
                            <div class="flex flex-col gap-1">
                                <div class="flex items-center gap-1">
                                    {#if getIconFromSearchResult(citation.source)}
                                        <img
                                            src={getIconFromSearchResult(citation.source)}
                                            alt=""
                                            class="!m-0 h-4 w-4 flex-shrink-0" />
                                    {:else}
                                        <FileText
                                            class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                    {/if}
                                    <h4 class="text-muted-foreground text-xs font-semibold">
                                        {getSourceDisplayName(
                                            inferSourceFromUrl(citation.source) ||
                                                SourceType.LOCAL_FILES,
                                        )}
                                    </h4>
                                </div>
                                <h4 class="truncate overflow-hidden text-sm font-semibold">
                                    {citation.title}
                                </h4>
                                <div
                                    class="text-muted-foreground overflow-hidden text-xs whitespace-break-spaces">
                                    {sanitizeCitedText(citation.cited_text)}
                                </div>
                            </div>
                        </HoverCard.Content>
                    </HoverCard.Root>
                {/if}
            {/each}
        </span>
    {/if}
{/snippet}

{#snippet sourcesSection(citations: TextCitationParam[])}
    {#if citations.length > 0}
        <div class="flex flex-col gap-1.5">
            <p class="text-muted-foreground text-xs font-bold uppercase">Sources</p>
            <div class="flex gap-1">
                {#each citations as citation}
                    {#if citation.type === 'search_result_location'}
                        <a
                            href={citation.source}
                            class="border-primary/10 hover:border-primary/20 hover:bg-muted/40 rounded-lg border p-2 px-2.5 text-xs font-normal no-underline transition-colors">
                            <div class="flex items-center gap-1">
                                {#if getIconFromSearchResult(citation.source)}
                                    <img
                                        src={getIconFromSearchResult(citation.source)}
                                        alt=""
                                        class="!m-0 h-4 w-4 flex-shrink-0" />
                                {:else}
                                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {/if}
                                <h1 class="text-muted-foreground text-sm font-semibold">
                                    {citation.title}
                                </h1>
                            </div>
                        </a>
                    {/if}
                {/each}
            </div>
        </div>
    {/if}
{/snippet}

<div class="flex h-full flex-col">
    <!-- Chat Container -->
    <div bind:this={chatContainerRef} class="flex w-full flex-1 flex-col overflow-y-auto px-4 pt-6">
        <div class="mx-auto mb-20 flex w-full max-w-4xl flex-1 flex-col gap-6">
            <!-- Existing Messages -->
            {#each processedMessages as message, i (message.id)}
                {#if message.role === 'user'}
                    <!-- User Message -->
                    {#if i === processedMessages.length - 1}
                        <div class="flex" bind:this={lastUserMessageRef}>
                            <div
                                class="text-foreground max-w-[80%] rounded-2xl bg-gray-200 px-6 py-4">
                                {@html marked.parse(
                                    (message.content[0] as TextMessageContent).text,
                                )}
                            </div>
                        </div>
                    {:else}
                        <div class="flex">
                            <div
                                class="text-foreground max-w-[80%] rounded-2xl bg-gray-200 px-6 py-4">
                                {@html marked.parse(
                                    (message.content[0] as TextMessageContent).text,
                                )}
                            </div>
                        </div>
                    {/if}
                {:else if message.role === 'assistant'}
                    <!-- Assistant Message -->
                    <div class="flex flex-col gap-1">
                        <div class="prose prose-p:my-3 max-w-none">
                            {#each message.content as block (block.id)}
                                {#if block.type === 'text'}
                                    <MarkdownMessage
                                        content={stripThinkingContent(block.text, 'thinking')}
                                        citations={block.citations} />
                                {:else if block.type === 'tool'}
                                    <div class="mb-1">
                                        <ToolMessage message={block} />
                                    </div>
                                {/if}
                            {/each}
                        </div>
                        {@render sourcesSection(collectSources(message))}
                        {#if !(isStreaming && i === processedMessages.length - 1)}
                            {@render messageControls(message)}
                        {/if}
                    </div>
                {/if}
            {/each}

            <!-- Streaming AI Response -->
            {#if isStreaming || error}
                <div class="flex px-2">
                    {#if error}
                        <Alert.Root variant="destructive">
                            <CircleAlert />
                            <Alert.Title>{error}</Alert.Title>
                            <!-- <Alert.Description>{error}</Alert.Description> -->
                        </Alert.Root>
                    {:else if isStreaming}
                        <span class="mt-2 flex items-center gap-1">
                            <span class="thinking-dot"></span>
                        </span>
                    {/if}
                </div>
            {/if}
        </div>

        <!-- Input -->
        <div class="bg-background sticky bottom-0 flex justify-center pb-4">
            <UserInput
                bind:value={userMessage}
                inputMode="chat"
                onSubmit={handleSubmit}
                onInput={(v) => (userMessage = v)}
                modeSelectorEnabled={false}
                placeholders={{
                    chat: 'Ask a follow-up...',
                    search: 'Search for something else...',
                }}
                {isStreaming}
                maxWidth="max-w-4xl" />
        </div>
    </div>
</div>

<style>
    @keyframes pulse-dot {
        0%,
        100% {
            transform: scale(1);
        }
        50% {
            transform: scale(1.5);
        }
    }

    .thinking-dot {
        display: inline-block;
        width: 12px;
        height: 12px;
        background-color: currentColor;
        border-radius: 50%;
        animation: pulse-dot 1.4s ease-in-out infinite;
    }
</style>
