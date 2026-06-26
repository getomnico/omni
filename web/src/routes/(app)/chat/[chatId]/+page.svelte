<script lang="ts">
    import { toast } from 'svelte-sonner'
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
        Mail,
        Pencil,
        ChevronLeft,
        ChevronRight,
        RotateCcw,
    } from '@lucide/svelte'
    import { marked } from 'marked'
    import { onDestroy, onMount } from 'svelte'
    import type { PageProps } from './$types'
    import type {
        ProcessedMessage,
        TextMessageContent,
        ToolMessageContent,
        UploadMessageContent,
        MessageContent,
        ApprovalRequiredEvent,
        OAuthRequired,
        OAuthRequiredEvent,
        ToolResultReplacedEvent,
        OmniUploadBlock,
    } from '$lib/types/message'
    import { OmniToolResultKind, tryParseOmniEnvelope } from '$lib/types/omni-tool-result'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import ToolCallsGroup from '$lib/components/tool-calls-group.svelte'
    import { cn } from '$lib/utils'
    import type { ToolResultBlockParam } from '@anthropic-ai/sdk/resources'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import { type ChatMessage } from '$lib/server/db/schema'
    import type {
        CitationSearchResultLocationParam,
        ContentBlockParam,
    } from '@anthropic-ai/sdk/resources.js'
    import { afterNavigate, invalidate, invalidateAll } from '$app/navigation'
    import { page } from '$app/state'
    import UserInput from '$lib/components/user-input.svelte'
    import UploadChip from '$lib/components/upload-chip.svelte'
    import * as Alert from '$lib/components/ui/alert'
    import type { Attachment } from 'svelte/attachments'
    import * as HoverCard from '$lib/components/ui/hover-card'
    import { copyTextToClipboard } from '$lib/utils'
    import {
        getIconFromSearchResult,
        getSourceDisplayName,
        getSourceIconPath,
        inferSourceFromUrl,
    } from '$lib/utils/icons'
    import * as Card from '$lib/components/ui/card'
    import { SourceType } from '$lib/types'
    import MarkdownMessage from '$lib/components/markdown-message.svelte'
    import ImapCitationSource from '$lib/components/search-results/imap-citation-source.svelte'
    import { themeStore } from '$lib/themes/store.svelte'
    import { formatChatTimestamp } from '$lib/utils/datetime'

    let { data }: PageProps = $props()
    let chatMessages = $state<ChatMessage[]>([...data.messages])
    let uploadFilenames = $state<Record<string, string>>({ ...data.uploadFilenames })

    onDestroy(() => {
        eventSource?.close()
        eventSource = null
        activeStreamChatId = null
        clearReconnectState()
    })

    afterNavigate(() => {
        const keepActiveStream = eventSource !== null && activeStreamChatId === data.chat.id

        // Tear down an active stream only when it belongs to a different chat.
        // On first navigation into a newly-created chat, onMount may have already
        // opened the EventSource from page.state.stream; closing it here cancels
        // the browser request before the stream can start.
        if (!keepActiveStream) {
            if (eventSource) {
                eventSource.close()
                eventSource = null
            }
            clearReconnectState()
            activeStreamChatId = null
            isStreaming = false
            stopInProgress = false
            error = null
            errorDetail = null
            stopThinkingText()
            chatMessages = [...data.messages]
            branchSelections = {}
            activeStreamingMessageId = null
            editingMessageId = null
            uploadFilenames = { ...data.uploadFilenames }
            markChatMessagesChanged()
        }
    })

    let userMessage = $state('')

    type UserMessageBlock = OmniUploadBlock | TextBlockParam

    type PendingUpload = { id: string; filename: string; sizeBytes: number; uploading: boolean }
    type UploadResponse = {
        id: string
        filename: string
        content_type: string
        size_bytes: number
        created_at: string
    }
    let pendingUploads = $state<PendingUpload[]>([])
    let uploadInputEl: HTMLInputElement | undefined = $state()

    async function handleFilesSelected(files: FileList | null) {
        if (!files) return
        for (const file of Array.from(files)) {
            const placeholder: PendingUpload = {
                id: crypto.randomUUID(),
                filename: file.name,
                sizeBytes: file.size,
                uploading: true,
            }
            pendingUploads.push(placeholder)
            try {
                const fd = new FormData()
                fd.append('file', file)
                const resp = await fetch('/api/uploads', { method: 'POST', body: fd })
                if (!resp.ok) throw new Error('upload failed')
                const data = (await resp.json()) as UploadResponse
                const idx = pendingUploads.findIndex((u) => u.id === placeholder.id)
                if (idx >= 0) {
                    pendingUploads[idx] = {
                        id: data.id,
                        filename: data.filename,
                        sizeBytes: data.size_bytes,
                        uploading: false,
                    }
                }
            } catch (err) {
                console.error(err)
                pendingUploads = pendingUploads.filter((u) => u.id !== placeholder.id)
                toast.error(`Failed to upload ${file.name}`, {
                    classes: { title: 'break-all' },
                })
            }
        }
        if (uploadInputEl) uploadInputEl.value = ''
    }

    function removePendingUpload(id: string) {
        pendingUploads = pendingUploads.filter((u) => u.id !== id)
    }
    let chatContainerRef: HTMLDivElement
    let chatContentRef: HTMLDivElement
    let lastUserMessageRef: HTMLDivElement | null = $state(null)
    let userInputRef: ReturnType<typeof UserInput>

    let isStreaming = $state(false)
    let stopInProgress = $state(false)
    let error = $state<string | null>(null)
    let errorDetail = $state<string | null>(null)
    let eventSource: EventSource | null = $state(null)
    let activeStreamChatId: string | null = null

    // --- Stream resilience (reconnect after a backgrounded tab / transient drop) ---
    // The server keeps the run alive and buffered, so we reconnect from the last
    // received offset (SSE Last-Event-ID) and resume seamlessly instead of failing.
    let streamLastEventId: string | null = null
    let reconnectAttempts = 0
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null
    let streamWatchdog: ReturnType<typeof setInterval> | null = null
    let lastStreamEventAt = 0
    // Re-opens the active chat's stream from streamLastEventId; set by streamResponse.
    let reconnectStream: (() => void) | null = null
    const MAX_RECONNECT_ATTEMPTS = 6
    const STREAM_STALL_MS = 20000

    function clearReconnectState() {
        reconnectAttempts = 0
        streamLastEventId = null
        reconnectStream = null
        if (reconnectTimer) {
            clearTimeout(reconnectTimer)
            reconnectTimer = null
        }
        if (streamWatchdog) {
            clearInterval(streamWatchdog)
            streamWatchdog = null
        }
    }

    type StreamErrorPayload = {
        message: string
        provider?: string | null
        model?: string | null
        statusCode?: number | null
    }

    function streamErrorMessage(event: MessageEvent<string>): {
        message: string
        detail: string | null
    } {
        try {
            const payload = JSON.parse(event.data) as StreamErrorPayload
            const detailParts = [
                payload.provider,
                payload.model,
                payload.statusCode ? `HTTP ${payload.statusCode}` : null,
            ].filter((part): part is string => !!part)
            return {
                message: payload.message,
                detail: detailParts.length ? detailParts.join(' · ') : null,
            }
        } catch {
            return {
                message: event.data || 'Failed to generate response. Please try again.',
                detail: null,
            }
        }
    }

    const defaultVerbs = ['Thinking', 'Reasoning', 'Analyzing', 'Processing']
    const slowMessages = [
        'Still working',
        'This is taking a bit longer',
        'Almost there',
        'Still thinking',
    ]

    const toolVerbMap: Record<string, string[]> = {
        search_documents: ['Searching', 'Looking it up', 'Digging through results'],
        web_search: ['Searching the web', 'Checking public sources'],
        fetch_web_page: ['Fetching web page', 'Reading web page'],
        read_document: ['Reading document', 'Reviewing document'],
        search_people: ['Searching people', 'Checking directory'],
        tool_search: ['Searching tools', 'Finding tools'],
        load_tool: ['Loading tool', 'Preparing tool'],
        load_tool_set: ['Loading tool set', 'Preparing tools'],
        skill_search: ['Searching skills', 'Finding guidance'],
        load_skill: ['Loading skill', 'Reading guidance'],
        write_file: ['Writing file', 'Preparing file'],
        read_file: ['Reading file', 'Opening file'],
        run_bash: ['Running command', 'Executing'],
        run_python: ['Running code', 'Executing script'],
        present_artifact: ['Preparing result', 'Finalizing'],
        send_email: ['Sending email', 'Preparing email'],
    }

    let thinkingText = $state(defaultVerbs[0])
    let thinkingVerbIndex = 0
    let thinkingRotateInterval: ReturnType<typeof setInterval> | null = null
    let thinkingSlowTimer: ReturnType<typeof setTimeout> | null = null
    let lastToolContext: string | null = null

    function pickRandom(arr: string[]): string {
        return arr[Math.floor(Math.random() * arr.length)]
    }

    function getThinkingVerbs(): string[] {
        if (lastToolContext && toolVerbMap[lastToolContext]) {
            return toolVerbMap[lastToolContext]
        }
        return defaultVerbs
    }

    function updateThinkingForTool(toolName: string) {
        lastToolContext = toolName
        const verbs = toolVerbMap[toolName]
        if (verbs) {
            thinkingText = pickRandom(verbs)
        } else {
            thinkingText = 'Working'
        }
        // Reset the slow timer since we just got new activity
        if (thinkingSlowTimer) {
            clearTimeout(thinkingSlowTimer)
            thinkingSlowTimer = setTimeout(() => {
                if (thinkingRotateInterval) clearInterval(thinkingRotateInterval)
                thinkingRotateInterval = null
                thinkingText = pickRandom(slowMessages)
            }, 15000)
        }
    }

    function updateThinkingForText() {
        if (lastToolContext) {
            lastToolContext = null
            thinkingText = pickRandom(defaultVerbs)
        }
    }

    function startThinkingText() {
        lastToolContext = null
        thinkingVerbIndex = Math.floor(Math.random() * defaultVerbs.length)
        thinkingText = defaultVerbs[thinkingVerbIndex]
        thinkingRotateInterval = setInterval(() => {
            const verbs = getThinkingVerbs()
            thinkingVerbIndex = (thinkingVerbIndex + 1) % verbs.length
            thinkingText = verbs[thinkingVerbIndex]
        }, 4000)
        thinkingSlowTimer = setTimeout(() => {
            if (thinkingRotateInterval) clearInterval(thinkingRotateInterval)
            thinkingRotateInterval = null
            thinkingText = pickRandom(slowMessages)
        }, 15000)
    }

    function stopThinkingText() {
        if (thinkingRotateInterval) {
            clearInterval(thinkingRotateInterval)
            thinkingRotateInterval = null
        }
        if (thinkingSlowTimer) {
            clearTimeout(thinkingSlowTimer)
            thinkingSlowTimer = null
        }
        lastToolContext = null
    }

    let copiedMessageId = $state<number | null>(null)
    let copiedUrl = $state(false)
    let messageFeedback = $state<Record<string, 'upvote' | 'downvote'>>({})
    let pendingApproval = $state<ApprovalRequiredEvent | null>(null)
    // Live OAuth metadata indexed by tool_call_id. The SSE oauth_required
    // event is the only place we learn `provider_configured` and a friendly
    // source display name; the persisted envelope on its own can render the
    // card in degraded mode (provider treated as "configured" optimistically).
    let oauthEventByToolCallId = $state<Record<string, OAuthRequiredEvent>>({})
    let editingMessageId = $state<string | null>(null)
    let editingContent = $state('')
    // Tracks user's branch choices: parentId -> chosen childId
    let branchSelections = $state<Record<string, string>>({})
    let activeStreamingMessageId = $state<string | null>(null)
    let userHasScrolled = $state(false)
    let showTopShadow = $state(false)
    let bottomPadding = $state(80)

    let processedMessages = $state<ProcessedMessage[]>(processMessages(chatMessages))
    let processedMessagesRefreshScheduled = false
    let lastUserMessageIndex = $derived(processedMessages.findLastIndex((m) => m.role === 'user'))

    function refreshProcessedMessages() {
        processedMessages = processMessages(chatMessages)
    }

    function scheduleProcessedMessagesRefresh() {
        if (processedMessagesRefreshScheduled) return
        processedMessagesRefreshScheduled = true
        requestAnimationFrame(() => {
            processedMessagesRefreshScheduled = false
            refreshProcessedMessages()
        })
    }

    function hashString(value: string): string {
        let hash = 2166136261
        for (let i = 0; i < value.length; i++) {
            hash ^= value.charCodeAt(i)
            hash = Math.imul(hash, 16777619)
        }
        return (hash >>> 0).toString(36)
    }

    function messageContentRenderKey(content: MessageContent): string {
        // This is an invalidation signature for remounting ToolCallsGroup, not a
        // globally unique message identity. Keep it readable and bounded: hash
        // potentially large tool inputs, and let text/citation updates flow
        // through props without forcing a full group remount.
        return content
            .map((block, index) => {
                if (block.type === 'text') return `t:${index}`

                if (block.type === 'tool') {
                    const inputHash = hashString(JSON.stringify(block.toolUse.input ?? {}))
                    const toolState = block.oauthRequired
                        ? `o:${block.oauthRequired.status}`
                        : block.actionResult
                          ? `a:${block.actionResult.isError ? 1 : 0}`
                          : block.toolResult
                            ? `r:${block.toolResult.content.length}`
                            : 'p'
                    return `u:${index}:${block.toolUse.id}:${inputHash}:${toolState}`
                }

                return `${block.type}:${index}`
            })
            .join('|')
    }

    function markChatMessagesChanged() {
        scheduleProcessedMessagesRefresh()
    }

    async function copyMessageToClipboard(message: ProcessedMessage) {
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
                    if (toolBlock.actionResult) {
                        return toolBlock.actionResult.text
                    }
                }
                return ''
            })
            .filter((text) => text.length > 0)
            .join('\n\n')

        try {
            await copyTextToClipboard(content)
            copiedMessageId = message.id
            setTimeout(() => {
                copiedMessageId = null
            }, 2000)
        } catch (error) {
            console.error('Failed to copy message:', error)
            toast.error('Failed to copy message')
        }
    }

    async function copyCurrentUrlToClipboard() {
        try {
            await copyTextToClipboard(window.location.href)
            copiedUrl = true
            setTimeout(() => {
                copiedUrl = false
            }, 2000)
        } catch (error) {
            console.error('Failed to copy URL:', error)
            toast.error('Failed to copy link')
        }
    }

    async function handleStop() {
        if (stopInProgress) return

        // The run is decoupled from this connection, so closing the EventSource
        // is no longer enough — tell the server to actually stop generating.
        // Keep the stream open until omni-ai emits message_id/end_of_stream for
        // the partial assistant message it persisted on cancellation.
        const stopChatId = activeStreamChatId ?? data.chat.id
        stopInProgress = true
        error = null
        errorDetail = null
        stopThinkingText()

        try {
            const response = await fetch(`/api/chat/${stopChatId}/stop`, { method: 'POST' })
            if (!response.ok) {
                console.error('Failed to stop stream')
                stopInProgress = false
            }
        } catch (err) {
            console.error('Failed to stop stream:', err)
            stopInProgress = false
        }
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

    function getPathToMessage(messages: ChatMessage[], messageId: string): ChatMessage[] {
        const messageById = new Map(messages.map((message) => [message.id, message]))
        const path: ChatMessage[] = []
        let current = messageById.get(messageId)
        const seen = new Set<string>()

        while (current && !seen.has(current.id)) {
            seen.add(current.id)
            path.push(current)
            current = current.parentId ? messageById.get(current.parentId) : undefined
        }

        return path.reverse()
    }

    // Build the display path from the message tree based on branch selections
    function getDisplayPath(chatMessages: ChatMessage[]): ChatMessage[] {
        if (chatMessages.length === 0) return []

        if (activeStreamingMessageId) {
            const streamingPath = getPathToMessage(chatMessages, activeStreamingMessageId)
            if (streamingPath.length > 0) return streamingPath
        }

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

    function nextMessageSeqNum(messages: ChatMessage[]): number {
        return Math.max(0, ...messages.map((message) => message.messageSeqNum)) + 1
    }

    function branchSelectionKey(parentId: string | null | undefined): string {
        return parentId ?? '.root'
    }

    function selectBranch(parentId: string | null | undefined, messageId: string) {
        branchSelections[branchSelectionKey(parentId)] = messageId
    }

    function replaceMessageIdInBranchSelections(oldId: string, newId: string) {
        const nextBranchSelections = { ...branchSelections }
        if (nextBranchSelections[oldId] !== undefined) {
            nextBranchSelections[newId] = nextBranchSelections[oldId]
            delete nextBranchSelections[oldId]
        }
        for (const [parentId, selectedId] of Object.entries(nextBranchSelections)) {
            if (selectedId === oldId) {
                nextBranchSelections[parentId] = newId
            }
        }
        branchSelections = nextBranchSelections
    }

    function switchBranch(parentId: string | null, direction: 'prev' | 'next') {
        const parentKey = parentId ?? null
        const childrenMap = buildChildrenMap(chatMessages)

        const siblings = childrenMap.get(parentKey)
        if (!siblings || siblings.length <= 1) return

        const selectionKey = branchSelectionKey(parentKey)
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
        activeStreamingMessageId = null
        // Clear downstream selections so we follow the default (active) path from here
        clearDownstreamSelections(siblings[newIdx].id)
        refreshProcessedMessages()
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
        const parentKey = branchSelectionKey(origMsg?.parentId)

        const newUserMessage: ChatMessage = {
            id: messageId,
            chatId: data.chat.id,
            parentId: origMsg?.parentId ?? null,
            message: { role: 'user', content: newContent },
            contentText: newContent,
            messageSeqNum: nextMessageSeqNum(chatMessages),
            createdAt: new Date(),
        }
        chatMessages = [...chatMessages, newUserMessage]

        // Select the new branch
        branchSelections[parentKey] = messageId
        clearDownstreamSelections(messageId)
        markChatMessagesChanged()

        streamResponse(data.chat.id)
    }

    // Converts messages into a format that makes it easy to render the messages
    // E.g., combines multiple content blocks into a single content block, handles citations, etc.
    function processMessages(chatMessages: ChatMessage[]): ProcessedMessage[] {
        let result: ProcessedMessage[] = []
        const siblingInfo = computeSiblingInfo(chatMessages)
        const displayPath = getDisplayPath(chatMessages)

        const addMessage = (message: ProcessedMessage) => {
            const lastMessage = result[result.length - 1]
            const messageIndex =
                lastMessage && lastMessage.role === message.role ? result.length - 1 : result.length

            const sourceMessageIds =
                lastMessage && lastMessage.role === message.role
                    ? [...lastMessage.sourceMessageIds, ...message.sourceMessageIds]
                    : [...message.sourceMessageIds]

            let messageToUpdate: ProcessedMessage =
                lastMessage && lastMessage.role === message.role
                    ? {
                          ...lastMessage,
                          sourceMessageIds,
                          renderKey: sourceMessageIds.join('+'),
                          origMessageId: message.origMessageId,
                          parentMessageId: message.parentMessageId,
                          siblingIds: message.siblingIds,
                          siblingIndex: message.siblingIndex,
                          createdAt: message.createdAt,
                          content: [...lastMessage.content],
                      }
                    : {
                          id: result.length,
                          sourceMessageIds,
                          renderKey: sourceMessageIds.join('+'),
                          origMessageId: message.origMessageId,
                          role: message.role,
                          content: [] as MessageContent,
                          parentMessageId: message.parentMessageId,
                          siblingIds: message.siblingIds,
                          siblingIndex: message.siblingIndex,
                          createdAt: message.createdAt,
                      }

            result =
                messageIndex === result.length
                    ? [...result, messageToUpdate]
                    : [
                          ...result.slice(0, messageIndex),
                          messageToUpdate,
                          ...result.slice(messageIndex + 1),
                      ]

            for (const [blockIdx, block] of message.content.entries()) {
                const lastBlock = messageToUpdate.content[messageToUpdate.content.length - 1]
                const crossesMessageBoundary =
                    blockIdx === 0 && lastMessage && lastMessage.role === message.role
                const textSeparator = crossesMessageBoundary ? '\n\n' : ''
                const nextContent: MessageContent =
                    lastBlock && lastBlock.type === 'text' && block.type === 'text'
                        ? [
                              ...messageToUpdate.content.slice(0, -1),
                              {
                                  ...lastBlock,
                                  text: lastBlock.text + textSeparator + block.text,
                                  citations: block.citations
                                      ? [...(lastBlock.citations ?? []), ...block.citations]
                                      : lastBlock.citations
                                        ? [...lastBlock.citations]
                                        : undefined,
                              },
                          ]
                        : [
                              ...messageToUpdate.content,
                              {
                                  ...block,
                                  id: messageToUpdate.content.length,
                              },
                          ]

                messageToUpdate = {
                    ...messageToUpdate,
                    content: nextContent,
                }
                result = [
                    ...result.slice(0, messageIndex),
                    messageToUpdate,
                    ...result.slice(messageIndex + 1),
                ]
            }
        }

        // `toolResult` here is the search-shape variant ({title, source, source_type}
        // pulled from `search_result` content blocks). Search tools render that
        // shape; everything else surfaces output via actionResult / oauthRequired
        // and should stay as a compact status row.
        const SEARCH_TOOLS = new Set(['search_documents', 'web_search'])
        const updateToolBlock = (
            toolUseId: string,
            updateBlock: (block: ToolMessageContent) => ToolMessageContent,
        ) => {
            for (let messageIdx = 0; messageIdx < result.length; messageIdx++) {
                const message = result[messageIdx]
                if (message.role !== 'assistant') continue

                const blockIdx = message.content.findIndex(
                    (block) => block.type === 'tool' && block.toolUse.id === toolUseId,
                )
                if (blockIdx === -1) continue

                const block = message.content[blockIdx]
                if (block.type !== 'tool') return

                const nextMessage = {
                    ...message,
                    content: [
                        ...message.content.slice(0, blockIdx),
                        updateBlock(block),
                        ...message.content.slice(blockIdx + 1),
                    ],
                }
                result = [
                    ...result.slice(0, messageIdx),
                    nextMessage,
                    ...result.slice(messageIdx + 1),
                ]
                return
            }
        }

        const updateToolResults = (toolResult: ToolMessageContent['toolResult']) => {
            if (!toolResult) return
            updateToolBlock(toolResult.toolUseId, (block) =>
                SEARCH_TOOLS.has(block.toolUse.name) ? { ...block, toolResult } : block,
            )
        }

        const updateActionResult = (actionResult: {
            toolUseId: string
            text: string
            isError: boolean
        }) => {
            updateToolBlock(actionResult.toolUseId, (block) => ({ ...block, actionResult }))
        }

        const updateOAuthRequired = (toolUseId: string, oauthRequired: OAuthRequired) => {
            updateToolBlock(toolUseId, (block) => ({ ...block, oauthRequired }))
        }

        for (let i = 0; i < displayPath.length; i++) {
            const chatMsg = displayPath[i]
            const message = chatMsg.message
            const info = siblingInfo.get(chatMsg.id)
            const messageCitations: TextCitationParam[] = [] // All citations in this message

            if (isUserMessage(message)) {
                // User messages may contain text blocks plus omni_upload document/image blocks.
                const userMessageContent: MessageContent =
                    typeof message.content === 'string'
                        ? [{ id: 0, type: 'text', text: message.content }]
                        : (message.content as Array<ContentBlockParam | OmniUploadBlock>)
                              .map((b, bi): MessageContent[number] | null => {
                                  if (b.type === 'text') {
                                      return { id: bi, type: 'text', text: b.text }
                                  }
                                  if (
                                      (b.type === 'document' || b.type === 'image') &&
                                      'source' in b &&
                                      b.source.type === 'omni_upload'
                                  ) {
                                      return {
                                          id: bi,
                                          type: 'upload',
                                          uploadId: b.source.upload_id,
                                      }
                                  }
                                  return null
                              })
                              .filter((b): b is MessageContent[number] => b !== null)

                const processedUserMessage: ProcessedMessage = {
                    id: result.length,
                    sourceMessageIds: [chatMsg.id],
                    renderKey: chatMsg.id,
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
                    id: result.length,
                    sourceMessageIds: [chatMsg.id],
                    renderKey: chatMsg.id,
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
                            text: (() => {
                                // Anthropic inlines 【source】 markers into block.text when
                                // citations are enabled. Replace them with clean [source] so
                                // the readable IMAP label is not shown raw with unicode brackets.
                                const cleaned = block.text.replace(/【([^】]*)】/g, '[$1]')
                                return citationTxt ? `${cleaned} ${citationTxt}` : cleaned
                            })(),
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
                                    source_type: (r as any).source_type ?? null,
                                })),
                            })

                            // Extract text content for non-search tool results (e.g., present_artifact)
                            const textBlocks: TextBlockParam[] = Array.isArray(block.content)
                                ? block.content.filter(
                                      (b): b is TextBlockParam => b.type === 'text',
                                  )
                                : []
                            // First text block may carry an Omni envelope (OAuth-required
                            // prompt). Surface it as a typed UI variant; if it doesn't
                            // parse, fall through to the normal action-result path.
                            let oauthHandled = false
                            if (textBlocks.length > 0) {
                                const envelope = tryParseOmniEnvelope(textBlocks[0].text)
                                if (
                                    envelope &&
                                    envelope.omni_kind === OmniToolResultKind.OauthRequired
                                ) {
                                    const live = oauthEventByToolCallId[toolUseId]
                                    updateOAuthRequired(toolUseId, {
                                        sourceId: envelope.payload.source_id,
                                        sourceType: envelope.payload.source_type,
                                        sourceDisplayName:
                                            live?.source_display_name ??
                                            getSourceDisplayName(
                                                envelope.payload.source_type as SourceType,
                                            ) ??
                                            envelope.payload.source_type,
                                        provider: envelope.payload.provider,
                                        // Optimistic default on refresh; corrected by
                                        // the live SSE event when present, and the
                                        // card itself can re-check via /api/oauth/provider-status.
                                        providerConfigured: live?.provider_configured ?? true,
                                        oauthStartUrl: envelope.payload.oauth_start_url,
                                        status: 'pending',
                                    })
                                    oauthHandled = true
                                }
                            }
                            if (!oauthHandled && textBlocks.length > 0) {
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

        return result
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
        const userMsgTop = lastUserMessageRef.offsetTop - chatContainerRef.offsetTop - 24
        const contentHeight = chatContainerRef.scrollHeight - bottomPadding
        // Pad so that max scroll aligns the last user message near the top of the viewport (with some breathing room).
        // Minimum 48px so the final assistant response doesn't sit flush against the input box.
        bottomPadding = Math.max(48, userMsgTop + containerHeight - contentHeight)
    }

    function scrollUserMessageToTop() {
        requestAnimationFrame(() => {
            recalcBottomPadding()
            requestAnimationFrame(() => {
                scrollToBottom()
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

        // When returning to a backgrounded/frozen tab, the SSE connection is
        // often dead without an 'error' event firing. If we're still streaming,
        // reconnect from the last offset and resume.
        const handleVisibility = () => {
            if (
                document.visibilityState === 'visible' &&
                isStreaming &&
                activeStreamChatId === data.chat.id &&
                (!eventSource || eventSource.readyState === EventSource.CLOSED) &&
                !reconnectTimer
            ) {
                reconnectStream?.()
            }
        }
        document.addEventListener('visibilitychange', handleVisibility)

        return () => {
            chatContainerRef?.removeEventListener('scroll', handleScroll)
            resizeObserver.disconnect()
            document.removeEventListener('visibilitychange', handleVisibility)
        }
    })

    function streamResponse(chatId: string) {
        isStreaming = true
        activeStreamChatId = chatId
        activeStreamingMessageId = null
        error = null
        errorDetail = null
        startThinkingText()

        const toolUseStateByIndex = new Map<
            number,
            { id: string; name: string; inputJson: string }
        >()
        const pendingTempMessageIds: string[] = []
        const pendingPersistedMessageIds: string[] = []
        let tempMessageCounter = 0

        let streamCompleted = false
        let messageEventsReceived = 0
        reconnectAttempts = 0

        const nextTempMessageId = () => `temp-${Date.now()}-${tempMessageCounter++}`

        const replaceTempMessageId = (tempId: string, messageId: string) => {
            chatMessages = chatMessages.map((message) => {
                if (message.id === tempId) {
                    return { ...message, id: messageId }
                }
                if (message.parentId === tempId) {
                    return { ...message, parentId: messageId }
                }
                return message
            })
            if (activeStreamingMessageId === tempId) {
                activeStreamingMessageId = messageId
            }
            replaceMessageIdInBranchSelections(tempId, messageId)
            markChatMessagesChanged()
        }

        const trackTempMessage = (tempId: string) => {
            const persistedMessageId = pendingPersistedMessageIds.shift()
            if (persistedMessageId) {
                replaceTempMessageId(tempId, persistedMessageId)
                return
            }
            pendingTempMessageIds.push(tempId)
        }

        const applyPersistedMessageId = (messageId: string) => {
            const tempId = pendingTempMessageIds.shift()
            if (!tempId) {
                pendingPersistedMessageIds.push(messageId)
                return
            }
            replaceTempMessageId(tempId, messageId)
        }

        const collectStreamingResponse = (
            block:
                | ToolUseBlock
                | TextBlockParam
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

            const replaceLastMessage = (message: ChatMessage) => {
                chatMessages = [...chatMessages.slice(0, -1), message]
                markChatMessagesChanged()
            }

            if (block.type !== 'tool_result' && !Array.isArray(lastMessage.message.content)) {
                throw new Error(
                    'Cannot append streamed assistant content to non-array message content',
                )
            }

            const existingBlocks = Array.isArray(lastMessage.message.content)
                ? ([...lastMessage.message.content] as ContentBlockParam[])
                : []
            if (block.type === 'text') {
                if (blockIdx === undefined) {
                    throw new Error('blockIdx is required for text block')
                }
                if (blockIdx >= existingBlocks.length) {
                    existingBlocks.push({
                        type: 'text',
                        text: block.text,
                        citations: block.citations ? [...block.citations] : undefined,
                    })
                } else {
                    const existingBlock = existingBlocks[blockIdx]
                    if (existingBlock.type !== 'text') {
                        throw new Error(
                            `Error handling text block, existing block at index ${blockIdx} is not a text block`,
                        )
                    }
                    existingBlocks[blockIdx] = {
                        ...existingBlock,
                        text: existingBlock.text + block.text,
                        citations: block.citations
                            ? [...(existingBlock.citations ?? []), ...block.citations]
                            : existingBlock.citations,
                    }
                }
            } else if (block.type === 'text_delta') {
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
                    existingBlocks[blockIdx] = {
                        ...existingBlock,
                        text: existingBlock.text + block.text,
                    }
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
                    existingBlocks[blockIdx] = {
                        ...existingBlock,
                        citations: [...(existingBlock.citations ?? []), block.citation],
                    }
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
                    const existingToolUseIdx = existingBlocks.findIndex(
                        (b) => b.type === 'tool_use' && b.id === block.id,
                    )

                    if (existingToolUseIdx !== -1) {
                        const existingToolUse = existingBlocks[existingToolUseIdx] as ToolUseBlock
                        existingBlocks[existingToolUseIdx] = {
                            ...existingToolUse,
                            input: block.input,
                        }
                    } else {
                        existingBlocks.push({
                            type: 'tool_use',
                            id: block.id,
                            name: block.name,
                            input: block.input,
                        })
                    }
                }
            } else if (block.type === 'tool_result') {
                if (lastMessage.message.role === 'user') {
                    const blocks = lastMessage.message.content
                    if (Array.isArray(blocks)) {
                        replaceLastMessage({
                            ...lastMessage,
                            message: {
                                ...lastMessage.message,
                                content: [...blocks, block],
                            },
                        })
                    }
                } else {
                    const displayPath = getDisplayPath(chatMessages)
                    const toolParentId =
                        displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined
                    const toolResultMessage: ChatMessage = {
                        id: nextTempMessageId(),
                        chatId,
                        parentId: toolParentId ?? null,
                        message: {
                            role: 'user',
                            content: [block],
                        },
                        contentText: null,
                        messageSeqNum: nextMessageSeqNum(chatMessages),
                        createdAt: new Date(),
                    }
                    chatMessages = [...chatMessages, toolResultMessage]
                    activeStreamingMessageId = toolResultMessage.id
                    selectBranch(toolResultMessage.parentId, toolResultMessage.id)
                    trackTempMessage(toolResultMessage.id)
                    markChatMessagesChanged()
                }

                return
            }

            replaceLastMessage({
                ...lastMessage,
                message: {
                    ...lastMessage.message,
                    content: existingBlocks,
                },
            })
        }

        const openStream = (resumeFromId: string | null) => {
            // Close any stale EventSource before opening a new one. Without this,
            // the old connection's error event fires after openStream returns, calls
            // handleConnectionError which closes the *new* eventSource, and triggers
            // another reconnect loop.
            if (eventSource) {
                eventSource.close()
                eventSource = null
            }
            const base = `/api/chat/${chatId}/stream`
            eventSource = new EventSource(
                resumeFromId ? `${base}?last_event_id=${encodeURIComponent(resumeFromId)}` : base,
                { withCredentials: true },
            )

            eventSource.addEventListener('message_id', (event) => {
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
                applyPersistedMessageId(event.data)
            })

            eventSource.addEventListener('not_resumable', () => {
                // The buffered run is gone (expired or never existed). Reload the
                // persisted messages from the server and clear streaming state.
                streamCompleted = true
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                refreshProcessedMessages()
                stopThinkingText()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()
                invalidateAll()
            })

            eventSource.addEventListener('title', () => {
                invalidate('app:recent_chats') // This will force a re-fetch of recent chats and update the title in the sidebar
            })

            eventSource.addEventListener('title_error', () => {
                // Title generation is best-effort; answer streaming should not surface its failures.
            })

            eventSource.addEventListener('message', (event) => {
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
                try {
                    const data: MessageStreamEvent | ToolResultBlockParam = JSON.parse(event.data)
                    if (data.type === 'message_start') {
                        // Find the last message in current display path to use as parent
                        const displayPath = getDisplayPath(chatMessages)
                        const streamParentId =
                            displayPath.length > 0
                                ? displayPath[displayPath.length - 1].id
                                : undefined
                        const startedMessage: ChatMessage = {
                            id: nextTempMessageId(),
                            chatId,
                            parentId: streamParentId ?? null,
                            message: {
                                role: data.message.role,
                                content: data.message.content,
                            },
                            contentText: null,
                            messageSeqNum: nextMessageSeqNum(chatMessages),
                            createdAt: new Date(),
                        }
                        chatMessages = [...chatMessages, startedMessage]
                        activeStreamingMessageId = startedMessage.id
                        selectBranch(startedMessage.parentId, startedMessage.id)
                        trackTempMessage(startedMessage.id)
                        markChatMessagesChanged()
                    } else if (data.type === 'content_block_start') {
                        if (data.content_block.type === 'tool_use') {
                            collectStreamingResponse(data.content_block, data.index)
                            toolUseStateByIndex.set(data.index, {
                                id: data.content_block.id,
                                name: data.content_block.name,
                                inputJson: '',
                            })
                            updateThinkingForTool(data.content_block.name)
                        } else if (data.content_block.type === 'text') {
                            collectStreamingResponse(data.content_block, data.index)
                        }
                    } else if (data.type === 'content_block_delta') {
                        if (data.delta.type === 'text_delta' && data.delta.text) {
                            updateThinkingForText()
                            collectStreamingResponse(data.delta, data.index)
                        } else if (data.delta.type === 'citations_delta') {
                            collectStreamingResponse(data.delta, data.index)
                        } else if (data.delta.type === 'input_json_delta') {
                            const toolUseState = toolUseStateByIndex.get(data.index)
                            if (!toolUseState) {
                                console.warn(
                                    `Received input JSON delta for unknown tool block index ${data.index}`,
                                )
                                return
                            }

                            // Parse partial JSON to show search query if possible
                            toolUseState.inputJson += data.delta.partial_json
                            try {
                                const parsedInput = JSON.parse(toolUseState.inputJson)
                                collectStreamingResponse(
                                    {
                                        type: 'tool_use',
                                        id: toolUseState.id,
                                        name: toolUseState.name,
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
                    stopInProgress = false
                    activeStreamingMessageId = null
                    refreshProcessedMessages()
                    stopThinkingText()
                    requestAnimationFrame(() => recalcBottomPadding())
                } catch (err) {
                    console.error('Failed to parse approval_required event:', err)
                }
            })

            eventSource.addEventListener('oauth_required', (event) => {
                try {
                    const oauthData: OAuthRequiredEvent = JSON.parse(event.data)
                    oauthEventByToolCallId[oauthData.tool_call_id] = oauthData
                    isStreaming = false
                    stopInProgress = false
                    activeStreamingMessageId = null
                    refreshProcessedMessages()
                    stopThinkingText()
                    requestAnimationFrame(() => recalcBottomPadding())
                } catch (err) {
                    console.error('Failed to parse oauth_required event:', err)
                }
            })

            eventSource.addEventListener('tool_result_replaced', (event) => {
                try {
                    const data: ToolResultReplacedEvent = JSON.parse(event.data)
                    // Find the user-role chat message that holds the placeholder
                    // tool_result block and replace it with the real result. The
                    // envelope text is gone, so processMessages will stop
                    // producing the OAuth card on the next derivation tick.
                    for (const cm of chatMessages) {
                        if (cm.message.role !== 'user') continue
                        const content = cm.message.content
                        if (!Array.isArray(content)) continue
                        let replaced = false
                        const next: ContentBlockParam[] = content.map((b) => {
                            if (b.type === 'tool_result' && b.tool_use_id === data.tool_use_id) {
                                replaced = true
                                const replacement: ToolResultBlockParam = {
                                    type: 'tool_result',
                                    tool_use_id: data.tool_use_id,
                                    content: data.content as ToolResultBlockParam['content'],
                                    is_error: data.is_error,
                                }
                                return replacement
                            }
                            return b
                        })
                        if (replaced) {
                            cm.message = { ...cm.message, content: next }
                            delete oauthEventByToolCallId[data.tool_use_id]
                            chatMessages = [...chatMessages]
                            markChatMessagesChanged()
                            break
                        }
                    }
                } catch (err) {
                    console.error('Failed to parse tool_result_replaced event:', err)
                }
            })

            eventSource.addEventListener('end_of_stream', () => {
                streamCompleted = true
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                refreshProcessedMessages()
                stopThinkingText()
                requestAnimationFrame(() => recalcBottomPadding())
                userInputRef?.focus()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()

                if (messageEventsReceived === 0 && !error) {
                    error = 'Failed to generate response. Please try again.'
                }
            })

            const handleStreamError = (event: Event) => {
                streamCompleted = true
                if (event instanceof MessageEvent) {
                    const streamError = streamErrorMessage(event as MessageEvent<string>)
                    error = streamError.message
                    errorDetail = streamError.detail
                } else {
                    error = 'Failed to generate response. Please try again.'
                    errorDetail = null
                }
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                refreshProcessedMessages()
                stopThinkingText()
                requestAnimationFrame(() => recalcBottomPadding())
                userInputRef?.focus()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()
            }

            const handleConnectionError = () => {
                if (streamCompleted || !isStreaming) return

                // Transient drop (e.g. backgrounded tab): the server keeps the run
                // alive and buffered, so reconnect from our last offset with backoff
                // instead of failing. Only surface an error once the budget is spent.
                eventSource?.close()
                eventSource = null
                if (reconnectAttempts < MAX_RECONNECT_ATTEMPTS) {
                    reconnectAttempts += 1
                    const delay = Math.min(1000 * 2 ** (reconnectAttempts - 1), 15000)
                    if (reconnectTimer) clearTimeout(reconnectTimer)
                    reconnectTimer = setTimeout(() => {
                        reconnectTimer = null
                        if (!isStreaming || streamCompleted || activeStreamChatId !== chatId) return
                        reconnectStream?.()
                    }, delay)
                    return
                }

                error =
                    messageEventsReceived > 0
                        ? 'Response stream disconnected before it finished. Please try again.'
                        : 'Failed to connect to the response stream. Please try again.'
                errorDetail = null
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                refreshProcessedMessages()
                stopThinkingText()
                requestAnimationFrame(() => recalcBottomPadding())
                userInputRef?.focus()
                activeStreamChatId = null
                clearReconnectState()
            }

            eventSource.addEventListener('stream_error', handleStreamError)
            eventSource.addEventListener('error', handleConnectionError)
        }

        streamLastEventId = null
        lastStreamEventAt = Date.now()
        reconnectStream = () => openStream(streamLastEventId)
        openStream(null)

        // Safety net for the case where the browser never fires an 'error' event
        // after a freeze: if the stream stalls while its connection is not open,
        // reconnect from the last offset.
        if (streamWatchdog) clearInterval(streamWatchdog)
        streamWatchdog = setInterval(() => {
            if (!isStreaming || streamCompleted || activeStreamChatId !== chatId) return
            const stalled = Date.now() - lastStreamEventAt > STREAM_STALL_MS
            const notOpen = !eventSource || eventSource.readyState !== EventSource.OPEN
            if (stalled && notOpen && !reconnectTimer) reconnectStream?.()
        }, 5000)
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
        const readyAttachments = pendingUploads.filter((u) => !u.uploading)
        if (pendingUploads.some((u) => u.uploading)) {
            return
        }
        if (!userMsg && readyAttachments.length === 0) return

        const displayPath = getDisplayPath(chatMessages)
        const parentId = displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined

        const attachmentIds = readyAttachments.map((u) => u.id)

        userMessage = ''

        let response: Response
        try {
            response = await fetch(`/api/chat/${data.chat.id}/messages`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: userMsg,
                    role: 'user',
                    parentId,
                    attachmentIds,
                }),
            })
        } catch (err) {
            userMessage = userMsg
            console.error('Failed to send message to chat session', err)
            return
        }

        if (!response.ok) {
            userMessage = userMsg
            console.error('Failed to send message to chat session')
            return
        }

        const { messageId } = await response.json()

        let messageContent: string | UserMessageBlock[]
        if (attachmentIds.length > 0) {
            for (const up of pendingUploads) {
                uploadFilenames[up.id] = up.filename
            }
            const blocks: UserMessageBlock[] = attachmentIds.map((id) => ({
                type: 'document',
                source: { type: 'omni_upload', upload_id: id },
            }))
            if (userMsg) blocks.push({ type: 'text', text: userMsg })
            messageContent = blocks
        } else {
            messageContent = userMsg
        }

        // The DB column is typed as Anthropic's MessageParam; our custom omni_upload
        // source isn't part of that union, so narrow to MessageParam via unknown.
        const newUserMessage: ChatMessage = {
            id: messageId,
            chatId: data.chat.id,
            parentId: parentId ?? null,
            message: {
                role: 'user',
                content: messageContent,
            } as unknown as ChatMessage['message'],
            contentText: userMsg,
            messageSeqNum: nextMessageSeqNum(chatMessages),
            createdAt: new Date(),
        }
        chatMessages = [...chatMessages, newUserMessage]
        selectBranch(newUserMessage.parentId, newUserMessage.id)
        markChatMessagesChanged()

        pendingUploads = []
        userHasScrolled = false

        scrollUserMessageToTop()
        streamResponse(data.chat.id)
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
        return formatChatTimestamp(date, data.user.configuration)
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
        data-testid={`branch-nav-${message.origMessageId}`}
        class="text-muted-foreground flex items-center gap-0.5 text-xs opacity-0 transition-opacity group-hover:opacity-100">
        <Button
            data-testid="branch-prev"
            size="icon"
            variant="ghost"
            class="h-6 w-6 cursor-pointer"
            disabled={message.siblingIndex === 0}
            onclick={() => switchBranch(message.parentMessageId ?? null, 'prev')}>
            <ChevronLeft class="h-3.5 w-3.5" />
        </Button>
        <span data-testid="branch-position" class="min-w-[3ch] text-center"
            >{(message.siblingIndex ?? 0) + 1}/{message.siblingIds?.length ?? 1}</span>
        <Button
            data-testid="branch-next"
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
        {@const firstText = message.content.find((b): b is TextMessageContent => b.type === 'text')}
        {@const uploads = message.content.filter(
            (b): b is UploadMessageContent => b.type === 'upload',
        )}
        <div class="flex max-w-[80%] flex-col items-end gap-1">
            {#if uploads.length > 0}
                <div class="mb-2 flex flex-wrap justify-end gap-1">
                    {#each uploads as up (up.uploadId)}
                        <UploadChip filename={uploadFilenames[up.uploadId]} />
                    {/each}
                </div>
            {/if}
            {#if firstText}
                <div
                    class="bg-secondary text-secondary-foreground w-fit rounded-2xl px-6 py-4 text-sm md:text-base">
                    {@html marked.parse(firstText.text)}
                </div>
            {/if}
            <div class="mx-0.5 mt-1 flex items-center justify-end gap-1">
                {@render messageTimestamp(message)}
                {#if message.siblingIds && message.siblingIds.length > 1}
                    {@render branchNavigation(message)}
                {/if}
                {#if !isStreaming}
                    <Button
                        size="icon"
                        variant="ghost"
                        class="h-7 w-7 cursor-pointer opacity-0 transition-opacity group-hover:opacity-100"
                        onclick={() => handleEdit(message.origMessageId, firstText?.text ?? '')}>
                        <RotateCcw class="h-3.5 w-3.5" />
                    </Button>
                    <Button
                        size="icon"
                        variant="ghost"
                        class="h-7 w-7 cursor-pointer opacity-0 transition-opacity group-hover:opacity-100"
                        onclick={() => {
                            editingMessageId = message.origMessageId
                            editingContent = firstText?.text ?? ''
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
                    {#snippet child({ props })}
                        <Button
                            {...props}
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
                    {/snippet}
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
                        {#snippet child({ props })}
                            <Button
                                {...props}
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
                        {/snippet}
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
                        {#snippet child({ props })}
                            <Button
                                {...props}
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
                        {/snippet}
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
                    {#snippet child({ props })}
                        <Button
                            {...props}
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
                    {/snippet}
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
                        {@const hasUrl =
                            citation.source?.startsWith('http://') ||
                            citation.source?.startsWith('https://')}
                        {@const isImap = citation.source?.startsWith('imap:')}
                        <svelte:element
                            this={hasUrl ? 'a' : 'div'}
                            href={hasUrl ? citation.source : undefined}
                            class="border-primary/10 hover:border-primary/20 hover:bg-muted/40 rounded-lg border p-2 px-2.5 text-xs font-normal no-underline transition-colors"
                            target={hasUrl ? '_blank' : undefined}
                            rel={hasUrl ? 'noopener noreferrer' : undefined}>
                            <div class="flex items-center gap-1">
                                <div class="text-muted-foreground text-sm">[{idx}]</div>
                                {#if isImap}
                                    <Mail class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {:else if getIconFromSearchResult(citation.source)}
                                    <img
                                        src={getIconFromSearchResult(citation.source)}
                                        alt=""
                                        class="!m-0 h-4 w-4 flex-shrink-0" />
                                {:else}
                                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {/if}
                                <div class="flex flex-col gap-0.5">
                                    <h1 class="text-muted-foreground text-sm font-semibold">
                                        {citation.title}
                                    </h1>
                                    <ImapCitationSource source={citation.source} />
                                </div>
                            </div>
                        </svelte:element>
                    {/if}
                {/each}
            </div>
        </div>
    {/if}
{/snippet}

<div class="flex h-full flex-col">
    <!-- Chat Container -->
    <div class="relative flex-1 overflow-hidden">
        <div
            class={cn(
                'from-background pointer-events-none absolute inset-x-0 top-0 z-10 h-6 bg-gradient-to-b to-transparent transition-opacity duration-300',
                showTopShadow ? 'opacity-100' : 'opacity-0',
            )}>
        </div>
        <div
            bind:this={chatContainerRef}
            class="flex h-full w-full flex-col overflow-x-hidden overflow-y-auto px-4 pt-6">
            <div
                bind:this={chatContentRef}
                class="mx-auto flex w-full max-w-4xl min-w-0 flex-1 flex-col gap-1"
                style:padding-bottom="{bottomPadding}px">
                {#if data.agent}
                    <div
                        class="bg-muted/50 mb-4 flex items-center justify-between rounded-lg border px-4 py-2">
                        <div class="flex items-center gap-2 text-sm">
                            <span class="text-muted-foreground">Chatting with agent:</span>
                            <a
                                href="/agents/{data.agent.id}"
                                class="cursor-pointer font-medium hover:underline">
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
                {#each processedMessages as message, i (message.renderKey)}
                    {#if message.role === 'user'}
                        <!-- User Message -->
                        {#if i === lastUserMessageIndex}
                            <div
                                data-testid={`chat-message-${message.origMessageId}`}
                                class="group mt-8 flex w-full min-w-0 flex-col items-end"
                                bind:this={lastUserMessageRef}>
                                {@render userMessageContent(message)}
                            </div>
                        {:else}
                            <div
                                data-testid={`chat-message-${message.origMessageId}`}
                                class="group mt-8 flex w-full min-w-0 flex-col items-end">
                                {@render userMessageContent(message)}
                            </div>
                        {/if}
                    {:else if message.role === 'assistant'}
                        <!-- Assistant Message -->
                        <div
                            data-testid={`chat-message-${message.origMessageId}`}
                            class="group mt-8 flex w-full min-w-0 flex-col gap-1">
                            <div
                                class="prose prose-sm md:prose-base prose-p:my-3 prose-headings:text-foreground prose-p:text-foreground prose-li:text-foreground prose-strong:text-foreground prose-code:text-foreground prose-a:text-primary dark:prose-invert max-w-none min-w-0 overflow-x-auto">
                                {#key `${message.renderKey}:${messageContentRenderKey(message.content)}`}
                                    <ToolCallsGroup
                                        content={message.content}
                                        isStreaming={isStreaming &&
                                            i === processedMessages.length - 1}
                                        {stripThinkingContent}
                                        isAdmin={data.user.role === 'admin'}
                                        onOAuthComplete={() => streamResponse(data.chat.id)} />
                                {/key}
                            </div>
                            {#if error && i === processedMessages.length - 1}
                                <div class="flex px-2">
                                    <Alert.Root variant="destructive" title={error}>
                                        <CircleAlert />
                                        <Alert.Title>{error}</Alert.Title>
                                        {#if errorDetail}
                                            <Alert.Description>{errorDetail}</Alert.Description>
                                        {/if}
                                    </Alert.Root>
                                </div>
                            {/if}
                            {#if pendingApproval && i === processedMessages.length - 1}
                                {@const connectorName = pendingApproval.tool_name.split('__')[0]}
                                {@const actionName = pendingApproval.tool_name
                                    .split('__')
                                    .slice(1)
                                    .join('__')}
                                {@const connectorIcon = getSourceIconPath(connectorName)}
                                <Card.Root class="gap-0 overflow-hidden py-0">
                                    <!-- Header -->
                                    <Card.Header
                                        class="flex items-center gap-3 border-b px-5 py-3 [.border-b]:py-3">
                                        {#if connectorIcon}
                                            <img
                                                src={connectorIcon}
                                                alt={connectorName}
                                                class="h-7 w-7" />
                                        {/if}
                                        <div class="min-w-0 flex-1">
                                            <Card.Title class="text-sm">
                                                {getSourceDisplayName(
                                                    connectorName as SourceType,
                                                ) || connectorName}
                                            </Card.Title>
                                            <Card.Description class="text-xs">
                                                {actionName.replaceAll('_', ' ')}
                                            </Card.Description>
                                        </div>
                                        <div
                                            class="flex items-center gap-1.5 rounded-full bg-amber-100 px-2.5 py-1 dark:bg-amber-950">
                                            <span class="h-1.5 w-1.5 rounded-full bg-amber-500"
                                            ></span>
                                            <span
                                                class="text-[11px] font-medium text-amber-700 dark:text-amber-400"
                                                >Awaiting approval</span>
                                        </div>
                                    </Card.Header>

                                    <!-- Parameters -->
                                    <Card.Content class="px-5 py-4">
                                        <div
                                            class="grid grid-cols-[80px_1fr] items-start gap-x-4 gap-y-2.5 text-[13px]">
                                            {#each Object.entries(pendingApproval.tool_input) as [key, value]}
                                                <div class="text-muted-foreground">{key}</div>
                                                <div
                                                    class={typeof value === 'string' &&
                                                    value.length > 60
                                                        ? 'leading-relaxed'
                                                        : 'font-mono'}>
                                                    {value}
                                                </div>
                                            {/each}
                                        </div>
                                    </Card.Content>

                                    <!-- Footer -->
                                    <Card.Footer
                                        class="bg-muted/50 justify-end gap-2 border-t px-3 py-3 [.border-t]:py-3">
                                        <Button
                                            size="sm"
                                            variant="outline"
                                            class="cursor-pointer"
                                            onclick={() => handleApproval('denied')}>
                                            Deny
                                        </Button>
                                        <Button
                                            size="sm"
                                            variant="default"
                                            class="cursor-pointer"
                                            onclick={() => handleApproval('approved')}>
                                            <Check class="h-3 w-3" />
                                            Approve & send
                                        </Button>
                                    </Card.Footer>
                                </Card.Root>
                            {/if}
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
                                {#if !(isStreaming && i === processedMessages.length - 1) && !(error && i === processedMessages.length - 1)}
                                    {@render messageControls(message)}
                                {/if}
                            </div>
                        </div>
                    {/if}
                {/each}

                <!-- Streaming AI Response -->
                {#if isStreaming || (error && processedMessages[processedMessages.length - 1]?.role !== 'assistant')}
                    <div class="flex px-2">
                        {#if error}
                            <Alert.Root variant="destructive" title={error}>
                                <CircleAlert />
                                <Alert.Title>{error}</Alert.Title>
                                {#if errorDetail}
                                    <Alert.Description>{errorDetail}</Alert.Description>
                                {/if}
                            </Alert.Root>
                        {:else if isStreaming}
                            <span class="thinking-container mt-2 flex items-center gap-1.5">
                                <img
                                    src={themeStore.current.omniLogoLight}
                                    alt="Thinking"
                                    class="omni-logo-light thinking-logo rounded opacity-60"
                                    width="20"
                                    height="20" />
                                <img
                                    src={themeStore.current.omniLogoDark}
                                    alt="Thinking"
                                    class="omni-logo-dark thinking-logo rounded opacity-60"
                                    width="20"
                                    height="20" />
                                <span class="text-muted-foreground text-sm">{thinkingText}...</span>
                            </span>
                        {/if}
                    </div>
                {/if}
            </div>

            {#snippet uploadChips()}
                {#if pendingUploads.length > 0}
                    <div class="flex flex-wrap gap-2">
                        {#each pendingUploads as up (up.id)}
                            <UploadChip
                                filename={up.filename}
                                uploading={up.uploading}
                                onRemove={() => removePendingUpload(up.id)} />
                        {/each}
                    </div>
                {/if}
            {/snippet}

            <!-- Input -->
            <div class="bg-background sticky bottom-0 flex flex-col items-center pb-4">
                <div class="w-full max-w-4xl">
                    <input
                        bind:this={uploadInputEl}
                        type="file"
                        multiple
                        class="hidden"
                        onchange={(e) =>
                            handleFilesSelected((e.target as HTMLInputElement).files)} />
                    <UserInput
                        bind:this={userInputRef}
                        bind:value={userMessage}
                        inputMode="chat"
                        onSubmit={handleSubmit}
                        onInput={(v) => (userMessage = v)}
                        onAttachClick={() => uploadInputEl?.click()}
                        onFilesDropped={(files) => handleFilesSelected(files)}
                        attachments={uploadChips}
                        modeSelectorEnabled={false}
                        placeholders={{
                            chat: 'Ask a follow-up...',
                            search: 'Search for something else...',
                        }}
                        isStreaming={isStreaming || stopInProgress}
                        onStop={handleStop}
                        maxWidth="max-w-4xl" />
                </div>
            </div>
        </div>
    </div>
</div>

<style>
    @keyframes shine-sweep {
        0% {
            left: -100%;
        }
        100% {
            left: 200%;
        }
    }

    .thinking-container {
        position: relative;
        overflow: hidden;
    }

    .thinking-container::after {
        content: '';
        position: absolute;
        top: 0;
        left: -100%;
        width: 50%;
        height: 100%;
        background: linear-gradient(
            120deg,
            transparent 0%,
            rgba(255, 255, 255, 0.6) 50%,
            transparent 100%
        );
        animation: shine-sweep 2s ease-in-out infinite;
        pointer-events: none;
    }

    :global(.dark) .thinking-container::after {
        background: linear-gradient(
            120deg,
            transparent 0%,
            rgba(255, 255, 255, 0.3) 50%,
            transparent 100%
        );
    }
</style>
