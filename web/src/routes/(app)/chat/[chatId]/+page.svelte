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
        Pencil,
        ChevronLeft,
        ChevronRight,
        RotateCcw,
    } from '@lucide/svelte'
    import { marked } from 'marked'
    import { onMount } from 'svelte'
    import type { PageProps } from './$types'
    import type {
        ProcessedMessage,
        TextMessageContent,
        ToolMessageContent,
        MessageContent,
        ApprovalRequiredEvent,
    } from '$lib/types/message'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import ToolCallsGroup from '$lib/components/tool-calls-group.svelte'
    import { cn } from '$lib/utils'
    import type { ToolResultBlockParam } from '@anthropic-ai/sdk/resources'
    import { page } from '$app/state'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import { type ChatMessage } from '$lib/server/db/schema'
    import type {
        CitationSearchResultLocationParam,
        ContentBlockParam,
    } from '@anthropic-ai/sdk/resources.js'
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
        branchSelections = {}
        editingMessageId = null
    })

    let userMessage = $state('')
    let chatContainerRef: HTMLDivElement
    let chatContentRef: HTMLDivElement
    let lastUserMessageRef: HTMLDivElement | null = $state(null)
    let userInputRef: ReturnType<typeof UserInput>

    let isStreaming = $state(false)
    let error = $state<string | null>(null)
    let eventSource: EventSource | null = $state(null)

    let copiedMessageId = $state<number | null>(null)
    let copiedUrl = $state(false)
    let messageFeedback = $state<Record<string, 'upvote' | 'downvote'>>({})
    let pendingApproval = $state<ApprovalRequiredEvent | null>(null)
    let editingMessageId = $state<string | null>(null)
    let editingContent = $state('')
    // Tracks user's branch choices: parentId -> chosen childId
    let branchSelections = $state<Record<string, string>>({})
    let userHasScrolled = $state(false)
    let showTopShadow = $state(false)
    let bottomPadding = $state(80)

    let processedMessages = $derived(processMessages(chatMessages))
    let lastUserMessageIndex = $derived(processedMessages.findLastIndex((m) => m.role === 'user'))

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

    function handleStop() {
        if (eventSource) {
            eventSource.close()
            eventSource = null
        }
        isStreaming = false
        requestAnimationFrame(() => recalcBottomPadding())
        userInputRef?.focus()
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

    // Groups messages by parentId, sorted by seq num within each group
    function buildChildrenMap(messages: ChatMessage[]): Map<string | null, ChatMessage[]> {
        const childrenMap = new Map<string | null, ChatMessage[]>()
        for (const msg of messages) {
            const parentKey = msg.parentId ?? null
            if (!childrenMap.has(parentKey)) {
                childrenMap.set(parentKey, [])
            }
            childrenMap.get(parentKey)!.push(msg)
        }
        for (const children of childrenMap.values()) {
            children.sort((a, b) => a.messageSeqNum - b.messageSeqNum)
        }
        return childrenMap
    }

    // Build the display path from the message tree based on branch selections
    function getDisplayPath(chatMessages: ChatMessage[]): ChatMessage[] {
        if (chatMessages.length === 0) return []

        const childrenMap = buildChildrenMap(chatMessages)

        // Walk from root, choosing branches based on branchSelections or defaulting to the child with highest seq num
        const path: ChatMessage[] = []
        const roots = childrenMap.get(null) || []
        if (roots.length === 0) return []

        // Pick root (there should be only one, but default to highest seq num)
        let current: ChatMessage = branchSelections['.root']
            ? roots.find((r) => r.id === branchSelections['.root']) || roots[roots.length - 1]
            : roots[roots.length - 1]

        while (current) {
            path.push(current)
            const children = childrenMap.get(current.id)
            if (!children || children.length === 0) break

            const selectedChildId = branchSelections[current.id]
            if (selectedChildId) {
                const selected = children.find((c) => c.id === selectedChildId)
                current = selected || children[children.length - 1]
            } else {
                // Default: pick child with highest seq num (active branch)
                current = children[children.length - 1]
            }
        }

        return path
    }

    // Compute sibling info for each message in the display path
    function computeSiblingInfo(
        chatMessages: ChatMessage[],
    ): Map<string, { siblingIds: string[]; siblingIndex: number }> {
        const childrenMap = buildChildrenMap(chatMessages)

        const result = new Map<string, { siblingIds: string[]; siblingIndex: number }>()
        for (const [, siblings] of childrenMap) {
            const ids = siblings.map((s) => s.id)
            for (let i = 0; i < siblings.length; i++) {
                result.set(siblings[i].id, { siblingIds: ids, siblingIndex: i })
            }
        }
        return result
    }

    function switchBranch(parentId: string | null, direction: 'prev' | 'next') {
        const parentKey = parentId ?? null
        const childrenMap = buildChildrenMap(chatMessages)

        const siblings = childrenMap.get(parentKey)
        if (!siblings || siblings.length <= 1) return

        const selectionKey = parentKey === null ? '.root' : parentKey!
        const currentId = branchSelections[selectionKey]
        let currentIdx = currentId
            ? siblings.findIndex((s) => s.id === currentId)
            : siblings.length - 1
        if (currentIdx === -1) currentIdx = siblings.length - 1

        const newIdx =
            direction === 'prev'
                ? Math.max(0, currentIdx - 1)
                : Math.min(siblings.length - 1, currentIdx + 1)

        branchSelections[selectionKey] = siblings[newIdx].id
        // Clear downstream selections so we follow the default (active) path from here
        clearDownstreamSelections(siblings[newIdx].id)
    }

    function clearDownstreamSelections(fromId: string) {
        const childrenMap = buildChildrenMap(chatMessages)

        const queue = [fromId]
        while (queue.length > 0) {
            const id = queue.shift()!
            delete branchSelections[id]
            const children = childrenMap.get(id) || []
            for (const child of children) {
                queue.push(child.id)
            }
        }
    }

    async function handleEdit(origMessageId: string, newContent: string) {
        editingMessageId = null
        const response = await fetch(`/api/chat/${data.chat.id}/messages/${origMessageId}/edit`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ content: newContent }),
        })

        if (!response.ok) {
            console.error('Failed to edit message')
            return
        }

        const { messageId } = await response.json()

        // Find the original message's parent to set the branch selection
        const origMsg = chatMessages.find((m) => m.id === origMessageId)
        const parentKey = origMsg?.parentId ?? '.root'

        const newUserMessage: ChatMessage = {
            id: messageId,
            chatId: data.chat.id,
            parentId: origMsg?.parentId ?? null,
            message: { role: 'user', content: newContent },
            messageSeqNum: chatMessages.length + 1,
            createdAt: new Date(),
        }
        chatMessages.push(newUserMessage)

        // Select the new branch
        branchSelections[parentKey] = messageId
        clearDownstreamSelections(messageId)

        streamResponse(data.chat.id)
    }

    // Converts messages into a format that makes it easy to render the messages
    // E.g., combines multiple content blocks into a single content block, handles citations, etc.
    function processMessages(chatMessages: ChatMessage[]): ProcessedMessage[] {
        const processedMessages: ProcessedMessage[] = []
        const siblingInfo = computeSiblingInfo(chatMessages)
        const displayPath = getDisplayPath(chatMessages)

        const addMessage = (message: ProcessedMessage) => {
            const lastMessage = processedMessages[processedMessages.length - 1]
            let messageToUpdate: ProcessedMessage
            if (!lastMessage || lastMessage.role !== message.role) {
                const newMessage = {
                    id: processedMessages.length,
                    origMessageId: message.origMessageId,
                    role: message.role,
                    content: [] as MessageContent,
                    parentMessageId: message.parentMessageId,
                    siblingIds: message.siblingIds,
                    siblingIndex: message.siblingIndex,
                    createdAt: message.createdAt,
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
                    lastBlock.text += '\n\n' + block.text
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

        const updateActionResult = (actionResult: {
            toolUseId: string
            text: string
            isError: boolean
        }) => {
            for (const message of processedMessages) {
                if (message.role === 'assistant') {
                    for (const block of message.content) {
                        if (block.type === 'tool' && block.toolUse.id === actionResult.toolUseId) {
                            block.actionResult = actionResult
                            return
                        }
                    }
                }
            }
        }

        for (let i = 0; i < displayPath.length; i++) {
            const chatMsg = displayPath[i]
            const message = chatMsg.message
            const info = siblingInfo.get(chatMsg.id)
            const messageCitations: TextCitationParam[] = [] // All citations in this message

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
                    origMessageId: chatMsg.id,
                    role: 'user',
                    content: userMessageContent,
                    parentMessageId: chatMsg.parentId ?? undefined,
                    siblingIds: info?.siblingIds,
                    siblingIndex: info?.siblingIndex,
                    createdAt:
                        chatMsg.createdAt instanceof Date
                            ? chatMsg.createdAt
                            : new Date(chatMsg.createdAt),
                }

                addMessage(processedUserMessage)
            } else {
                // Here we handle both assistant messages (with possible tool uses) and also user messages that contain tool results
                const processedMessage: ProcessedMessage = {
                    id: processedMessages.length,
                    origMessageId: chatMsg.id,
                    role: 'assistant',
                    content: [],
                    parentMessageId: chatMsg.parentId ?? undefined,
                    siblingIds: info?.siblingIds,
                    siblingIndex: info?.siblingIndex,
                    createdAt:
                        chatMsg.createdAt instanceof Date
                            ? chatMsg.createdAt
                            : new Date(chatMsg.createdAt),
                }

                const contentBlocks = Array.isArray(message.content)
                    ? message.content
                    : [{ type: 'text', text: message.content } as TextBlockParam]

                for (let blockIdx = 0; blockIdx < contentBlocks.length; blockIdx++) {
                    const block = contentBlocks[blockIdx]
                    if (block.type === 'text') {
                        let citationTxt = ''
                        for (const citation of block.citations || []) {
                            if (citation.type === 'search_result_location') {
                                const existingCitationIdx = messageCitations.findIndex(
                                    (c) =>
                                        c.type === 'search_result_location' &&
                                        c.source === citation.source,
                                )
                                if (existingCitationIdx !== -1) {
                                    citationTxt += ` [${existingCitationIdx}]`
                                } else {
                                    const citationIdx = messageCitations.length
                                    messageCitations.push(citation)
                                    citationTxt += ` [${citationIdx}]`
                                }
                            }
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
                                      (b: any) => b.type === 'search_result',
                                  ) as SearchResultBlockParam[])
                                : []
                            updateToolResults({
                                toolUseId,
                                content: searchResults.map((r) => ({
                                    title: r.title,
                                    source: r.source,
                                })),
                            })

                            // Extract text content for non-search tool results (e.g., present_artifact)
                            const textBlocks = Array.isArray(block.content)
                                ? block.content.filter((b: any) => b.type === 'text')
                                : []
                            if (textBlocks.length > 0) {
                                const text = textBlocks.map((b: any) => b.text).join('\n')
                                updateActionResult({
                                    toolUseId,
                                    text,
                                    isError: block.is_error || false,
                                })
                            }
                        }
                    }
                }

                // Add a separate block containing all the citation links
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

    function recalcBottomPadding() {
        if (!lastUserMessageRef || !chatContainerRef) return
        const containerHeight = chatContainerRef.clientHeight
        const userMsgTop = lastUserMessageRef.offsetTop - chatContainerRef.offsetTop
        const contentHeight = chatContainerRef.scrollHeight - bottomPadding
        // Pad so that max scroll aligns the last user message near the top of the viewport
        bottomPadding = Math.max(0, userMsgTop + containerHeight - contentHeight)
    }

    function scrollUserMessageToTop() {
        requestAnimationFrame(() => {
            recalcBottomPadding()
            requestAnimationFrame(() => {
                if (lastUserMessageRef && chatContainerRef) {
                    const messageTop = lastUserMessageRef.offsetTop - chatContainerRef.offsetTop
                    chatContainerRef.scrollTo({ top: messageTop, behavior: 'smooth' })
                }
            })
        })
    }

    // This will trigger the streaming of AI response when the component is mounted
    // If no response is currently being streamed, nothing happens
    onMount(() => {
        if ((page.state as any).stream) {
            streamResponse(data.chat.id)
        }

        const handleScroll = () => {
            if (!chatContainerRef) return
            const { scrollTop, scrollHeight, clientHeight } = chatContainerRef
            const isNearBottom = scrollTop + clientHeight >= scrollHeight - 100
            userHasScrolled = !isNearBottom
            showTopShadow = scrollTop > 0
        }
        chatContainerRef?.addEventListener('scroll', handleScroll)

        const resizeObserver = new ResizeObserver(() => recalcBottomPadding())
        if (chatContentRef) resizeObserver.observe(chatContentRef)

        return () => {
            chatContainerRef?.removeEventListener('scroll', handleScroll)
            resizeObserver.disconnect()
        }
    })

    function streamResponse(chatId: string) {
        isStreaming = true
        error = null

        let currToolUseId: string
        let currToolUseName: string
        let currToolUseInputStr: string

        eventSource = new EventSource(`/api/chat/${chatId}/stream`, { withCredentials: true })

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
                    const displayPath = getDisplayPath(chatMessages)
                    const toolParentId =
                        displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined
                    chatMessages.push({
                        id: `temp-${Date.now()}`,
                        chatId,
                        parentId: toolParentId ?? null,
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
                    // Find the last message in current display path to use as parent
                    const displayPath = getDisplayPath(chatMessages)
                    const streamParentId =
                        displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined
                    chatMessages.push({
                        id: `temp-${Date.now()}`,
                        chatId,
                        parentId: streamParentId ?? null,
                        message: {
                            role: data.message.role,
                            content: data.message.content,
                        },
                        messageSeqNum: chatMessages.length + 1,
                        createdAt: new Date(),
                    })
                } else if (data.type === 'content_block_start') {
                    if (data.content_block.type === 'tool_use') {
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

                if (!userHasScrolled) scrollToBottom()
            } catch (err) {
                console.error('Failed to parse SSE data:', event.data, err)
            } finally {
                messageEventsReceived += 1
            }
        })

        eventSource.addEventListener('approval_required', (event) => {
            try {
                const approvalData: ApprovalRequiredEvent = JSON.parse(event.data)
                pendingApproval = approvalData
                isStreaming = false
                requestAnimationFrame(() => recalcBottomPadding())
            } catch (err) {
                console.error('Failed to parse approval_required event:', err)
            }
        })

        eventSource.addEventListener('end_of_stream', () => {
            streamCompleted = true
            isStreaming = false
            requestAnimationFrame(() => recalcBottomPadding())
            userInputRef?.focus()
            eventSource?.close()
            eventSource = null

            if (messageEventsReceived === 0 && !error) {
                error = 'Failed to generate response. Please try again.'
            }
        })

        eventSource.addEventListener('error', (event) => {
            error = 'Failed to generate response. Please try again.'
            isStreaming = false
            requestAnimationFrame(() => recalcBottomPadding())
            userInputRef?.focus()
            eventSource?.close()
            eventSource = null
        })
    }

    async function handleApproval(decision: 'approved' | 'denied') {
        if (!pendingApproval) return

        try {
            const response = await fetch(`/api/chat/${data.chat.id}/approve`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    approvalId: pendingApproval.approval_id,
                    decision,
                }),
            })

            if (!response.ok) {
                console.error('Failed to submit approval decision')
                return
            }

            pendingApproval = null

            if (decision === 'approved') {
                // Re-trigger stream to resume execution
                streamResponse(data.chat.id)
            }
        } catch (err) {
            console.error('Error submitting approval:', err)
        }
    }

    async function handleSubmit() {
        const userMsg = userMessage.trim()
        if (userMsg) {
            // Determine parentId: last message in current display path
            const displayPath = getDisplayPath(chatMessages)
            const parentId =
                displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined

            const response = await fetch(`/api/chat/${data.chat.id}/messages`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: userMsg,
                    role: 'user',
                    parentId,
                }),
            })

            if (!response.ok) {
                console.error('Failed to send message to chat session')
                return
            }

            const { messageId } = await response.json()

            const newUserMessage: ChatMessage = {
                id: messageId,
                chatId: data.chat.id,
                parentId: parentId ?? null,
                message: {
                    role: 'user',
                    content: userMsg,
                },
                messageSeqNum: chatMessages.length + 1,
                createdAt: new Date(),
            }
            chatMessages.push(newUserMessage)

            userMessage = ''
            userHasScrolled = false

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

    function formatMessageTimestamp(date: Date): string {
        const now = new Date()
        const isToday =
            date.getDate() === now.getDate() &&
            date.getMonth() === now.getMonth() &&
            date.getFullYear() === now.getFullYear()

        if (isToday) {
            return date
                .toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit', hour12: true })
                .toLowerCase()
        }
        return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
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

{#snippet branchNavigation(message: ProcessedMessage)}
    <div
        class="text-muted-foreground flex items-center gap-0.5 text-xs opacity-0 transition-opacity group-hover:opacity-100">
        <Button
            size="icon"
            variant="ghost"
            class="h-6 w-6 cursor-pointer"
            disabled={message.siblingIndex === 0}
            onclick={() => switchBranch(message.parentMessageId ?? null, 'prev')}>
            <ChevronLeft class="h-3.5 w-3.5" />
        </Button>
        <span class="min-w-[3ch] text-center"
            >{(message.siblingIndex ?? 0) + 1}/{message.siblingIds?.length ?? 1}</span>
        <Button
            size="icon"
            variant="ghost"
            class="h-6 w-6 cursor-pointer"
            disabled={message.siblingIndex === (message.siblingIds?.length ?? 1) - 1}
            onclick={() => switchBranch(message.parentMessageId ?? null, 'next')}>
            <ChevronRight class="h-3.5 w-3.5" />
        </Button>
    </div>
{/snippet}

{#snippet messageTimestamp(message: ProcessedMessage)}
    {#if message.createdAt}
        <span
            class="text-muted-foreground text-xs opacity-0 transition-opacity group-hover:opacity-100">
            {formatMessageTimestamp(message.createdAt)}
        </span>
    {/if}
{/snippet}

{#snippet userMessageContent(message: ProcessedMessage)}
    {#if editingMessageId === message.origMessageId}
        <div class="w-full max-w-[80%]">
            <textarea
                class="border-border bg-card w-full resize-none rounded-2xl border px-6 py-4 text-sm focus:outline-none"
                rows={3}
                bind:value={editingContent}
                onkeydown={(e) => {
                    if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault()
                        handleEdit(message.origMessageId, editingContent)
                    }
                }}></textarea>
            <div class="mt-1 flex justify-end gap-1">
                <Button
                    size="sm"
                    class="cursor-pointer"
                    onclick={() => handleEdit(message.origMessageId, editingContent)}>
                    Submit
                </Button>
                <Button
                    size="sm"
                    variant="outline"
                    class="cursor-pointer"
                    onclick={() => (editingMessageId = null)}>
                    Cancel
                </Button>
            </div>
        </div>
    {:else}
        <div class="relative max-w-[80%]">
            <div class="text-foreground w-fit rounded-2xl bg-gray-200 px-6 py-4">
                {@html marked.parse((message.content[0] as TextMessageContent).text)}
            </div>
            <div class="absolute right-0 mx-0.5 mt-1 flex items-center justify-end gap-1">
                {@render messageTimestamp(message)}
                {#if message.siblingIds && message.siblingIds.length > 1}
                    {@render branchNavigation(message)}
                {/if}
                {#if !isStreaming}
                    <Button
                        size="icon"
                        variant="ghost"
                        class="h-7 w-7 cursor-pointer opacity-0 transition-opacity group-hover:opacity-100"
                        onclick={() =>
                            handleEdit(
                                message.origMessageId,
                                (message.content[0] as TextMessageContent).text,
                            )}>
                        <RotateCcw class="h-3.5 w-3.5" />
                    </Button>
                    <Button
                        size="icon"
                        variant="ghost"
                        class="h-7 w-7 cursor-pointer opacity-0 transition-opacity group-hover:opacity-100"
                        onclick={() => {
                            editingMessageId = message.origMessageId
                            editingContent = (message.content[0] as TextMessageContent).text
                        }}>
                        <Pencil class="h-3.5 w-3.5" />
                    </Button>
                    <Button
                        size="icon"
                        variant="ghost"
                        class="h-7 w-7 cursor-pointer opacity-0 transition-opacity group-hover:opacity-100"
                        onclick={() => copyMessageToClipboard(message)}>
                        {#if copiedMessageId === message.id}
                            <Check class="h-3.5 w-3.5 text-green-600" />
                        {:else}
                            <Copy class="h-3.5 w-3.5" />
                        {/if}
                    </Button>
                {/if}
            </div>
        </div>
    {/if}
{/snippet}

{#snippet messageControls(message: ProcessedMessage)}
    <div class="flex items-center justify-start gap-0.5" data-role="message-controls">
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

{#snippet sourcesSection(citations: TextCitationParam[])}
    {#if citations.length > 0}
        <div class="flex flex-col gap-1.5">
            <p class="text-muted-foreground pl-1 text-xs font-bold uppercase">Sources</p>
            <div class="flex flex-wrap gap-1">
                {#each citations as citation, idx}
                    {#if citation.type === 'search_result_location'}
                        <a
                            href={citation.source}
                            class="border-primary/10 hover:border-primary/20 hover:bg-muted/40 rounded-lg border p-2 px-2.5 text-xs font-normal no-underline transition-colors"
                            target="_blank"
                            rel="noopener noreferrer">
                            <div class="flex items-center gap-1">
                                <div class="text-muted-foreground text-sm">[{idx}]</div>
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
    <div class="relative flex-1 overflow-hidden">
        {#if showTopShadow}
            <div
                class="pointer-events-none absolute inset-x-0 top-0 z-10 h-8 bg-gradient-to-b from-black/[0.07] to-transparent">
            </div>
        {/if}
        <div
            bind:this={chatContainerRef}
            class="flex h-full w-full flex-col overflow-y-auto px-4 pt-6">
            <div
                bind:this={chatContentRef}
                class="mx-auto flex w-full max-w-4xl flex-1 flex-col gap-1"
                style:padding-bottom="{bottomPadding}px">
                {#if data.agent}
                    <div class="bg-muted/50 mb-4 flex items-center justify-between rounded-lg border px-4 py-2">
                        <div class="flex items-center gap-2 text-sm">
                            <span class="text-muted-foreground">Chatting with agent:</span>
                            <a href="/agents/{data.agent.id}" class="font-medium hover:underline cursor-pointer">
                                {data.agent.name}
                            </a>
                        </div>
                        <span class="text-muted-foreground text-xs">Read-only session</span>
                    </div>
                {/if}
                {#if data.modelDisplayName}
                    <div class="flex justify-center">
                        <span class="text-muted-foreground rounded-full border px-3 py-0.5 text-xs">
                            {data.modelDisplayName}
                        </span>
                    </div>
                {/if}
                <!-- Existing Messages -->
                {#each processedMessages as message, i (message.id)}
                    {#if message.role === 'user'}
                        <!-- User Message -->
                        {#if i === lastUserMessageIndex}
                            <div
                                class="group mt-8 flex flex-col items-end"
                                bind:this={lastUserMessageRef}>
                                {@render userMessageContent(message)}
                            </div>
                        {:else}
                            <div class="group mt-8 flex flex-col items-end">
                                {@render userMessageContent(message)}
                            </div>
                        {/if}
                    {:else if message.role === 'assistant'}
                        <!-- Assistant Message -->
                        <div class="group mt-8 flex flex-col gap-1">
                            <div class="prose prose-p:my-3 max-w-none">
                                <ToolCallsGroup
                                    content={message.content}
                                    isStreaming={isStreaming && i === processedMessages.length - 1}
                                    {stripThinkingContent} />
                            </div>
                            {#if !isStreaming}
                                {@render sourcesSection(collectSources(message))}
                            {/if}
                            <div
                                class={cn(
                                    'flex items-center gap-1',
                                    i !== processedMessages.length - 1 &&
                                        'opacity-0 transition-opacity group-hover:opacity-100',
                                )}>
                                {#if message.siblingIds && message.siblingIds.length > 1}
                                    {@render branchNavigation(message)}
                                {/if}
                                {#if !(isStreaming && i === processedMessages.length - 1)}
                                    {@render messageControls(message)}
                                {/if}
                            </div>
                        </div>
                    {/if}
                {/each}

                <!-- Approval Required UI -->
                {#if pendingApproval}
                    <div class="mx-auto w-full max-w-lg">
                        <div class="rounded-lg border border-amber-200 bg-amber-50 p-4">
                            <div class="mb-2 flex items-center gap-2">
                                <CircleAlertIcon class="h-5 w-5 text-amber-600" />
                                <h3 class="text-sm font-semibold text-amber-800">
                                    Action requires approval
                                </h3>
                            </div>
                            <div class="mb-3 space-y-1">
                                <p class="text-sm text-amber-700">
                                    <span class="font-medium"
                                        >{pendingApproval.tool_name.replace('__', ' > ')}</span>
                                </p>
                                <pre
                                    class="max-h-32 overflow-auto rounded bg-amber-100 p-2 text-xs text-amber-900">{JSON.stringify(
                                        pendingApproval.tool_input,
                                        null,
                                        2,
                                    )}</pre>
                            </div>
                            <div class="flex gap-2">
                                <Button
                                    size="sm"
                                    variant="default"
                                    class="cursor-pointer bg-green-600 text-white hover:bg-green-700"
                                    onclick={() => handleApproval('approved')}>
                                    Approve
                                </Button>
                                <Button
                                    size="sm"
                                    variant="outline"
                                    class="cursor-pointer"
                                    onclick={() => handleApproval('denied')}>
                                    Deny
                                </Button>
                            </div>
                        </div>
                    </div>
                {/if}

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
                    bind:this={userInputRef}
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
                    onStop={handleStop}
                    maxWidth="max-w-4xl" />
            </div>
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
