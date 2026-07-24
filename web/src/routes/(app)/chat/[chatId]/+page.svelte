<script lang="ts">
    import { toast } from 'svelte-sonner'
    import { Button } from '$lib/components/ui/button'
    import type {
        MessageParam,
        MessageStreamEvent,
        SearchResultBlockParam,
        TextBlockParam,
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
        Globe,
        ExternalLink,
        FileText,
        Mail,
        Pencil,
        ChevronLeft,
        ChevronRight,
        RotateCcw,
        ArrowDown,
        BookOpen,
        X,
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
        OmniUploadBlock,
    } from '$lib/types/message'
    import { ToolApprovalStatus } from '$lib/types/message'
    import { OmniToolResultKind, tryParseOmniEnvelope } from '$lib/types/omni-tool-result'
    import { fetchChatStreamStatus } from '$lib/utils/stream-status'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import ToolCallsGroup from '$lib/components/tool-calls-group.svelte'
    import { cn } from '$lib/utils'
    import type { ToolResultBlockParam } from '@anthropic-ai/sdk/resources'
    import * as Tooltip from '$lib/components/ui/tooltip'
    import { type ChatMessage } from '$lib/server/db/schema'
    import type { ContentBlockParam } from '@anthropic-ai/sdk/resources.js'
    import {
        normalizeCitation,
        citationIdFromCitation,
        sourceIdentityFromCitation,
        type NormalizedCitation,
    } from '$lib/utils/citations'
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
    import * as Drawer from '$lib/components/ui/drawer'
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
            pendingApproval = pendingApprovalFromData()
            oauthEventByToolCallId = {}
            oauthBlockerActive = data.pendingOAuth !== null
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
    let streamRunCounter = 0

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

    async function resumeActiveStreamIfNeeded() {
        if (isStreaming || eventSource) return
        try {
            const status = await fetchChatStreamStatus(data.chat.id)
            if (status?.running) {
                streamResponse(data.chat.id)
            }
        } catch (err) {
            console.warn('Failed to check chat stream status', err)
        }
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
    let isCompacting = $state(false)
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
        isCompacting = false
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
        isCompacting = false
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
    function pendingApprovalFromData(): ApprovalRequiredEvent | null {
        const approvals = data.pendingApprovals.length > 0 ? data.pendingApprovals : []
        const first = approvals[0]
        if (!first) return null

        const mapped = approvals.map((approval) => ({
            approval_id: approval.id,
            tool_name: approval.toolName,
            tool_input: approval.toolInput as Record<string, unknown>,
            tool_call_id: approval.toolCallId ?? '',
            source_id: approval.sourceId,
            source_type: approval.sourceType,
        }))

        return {
            ...mapped[0],
            approvals: mapped,
        }
    }

    let pendingApproval = $state<ApprovalRequiredEvent | null>(pendingApprovalFromData())

    type ApprovalInputDisplayField = {
        label: string
        value: string
    }

    function humanizeApprovalToken(value: string): string {
        return value
            .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
            .replace(/[_-]+/g, ' ')
            .replace(/\b\w/g, (match) => match.toUpperCase())
            .replace(/\bId\b/g, 'ID')
            .trim()
    }

    function approvalActionLabel(toolName: string): string {
        const actionName = toolName.split('__').slice(1).join('__') || toolName
        return humanizeApprovalToken(actionName).toLowerCase()
    }

    function isApprovalRecord(value: unknown): value is Record<string, unknown> {
        return typeof value === 'object' && value !== null && !Array.isArray(value)
    }

    function formatApprovalDisplayValue(value: unknown): string {
        if (value === null || value === undefined) return 'Not provided'
        if (typeof value === 'boolean') return value ? 'Yes' : 'No'
        if (typeof value === 'number') return String(value)
        if (typeof value === 'string') return value || 'Not provided'
        if (Array.isArray(value)) {
            if (value.length === 0) return 'None'
            if (value.every((item) => ['string', 'number', 'boolean'].includes(typeof item))) {
                return value.map(formatApprovalDisplayValue).join(', ')
            }
            return `${value.length} ${value.length === 1 ? 'item' : 'items'}`
        }
        if (isApprovalRecord(value)) {
            const entries = Object.entries(value).filter(
                ([, entryValue]) => entryValue !== undefined,
            )
            if (entries.length === 0) return 'None'
            return entries
                .slice(0, 4)
                .map(
                    ([key, entryValue]) =>
                        `${humanizeApprovalToken(key)}: ${formatApprovalDisplayValue(entryValue)}`,
                )
                .join(' · ')
        }
        return String(value)
    }

    function approvalInputDisplayFields(
        input: Record<string, unknown>,
    ): ApprovalInputDisplayField[] {
        return Object.entries(input)
            .filter(([, value]) => value !== undefined)
            .map(([key, value]) => ({
                label: humanizeApprovalToken(key),
                value: formatApprovalDisplayValue(value),
            }))
    }

    function formatApprovalTechnicalDetails(input: Record<string, unknown>): string {
        try {
            return JSON.stringify(input, null, 2)
        } catch {
            return String(input)
        }
    }

    // Live OAuth metadata indexed by tool_call_id. The SSE oauth_required
    // event is the only place we learn `provider_configured` and a friendly
    // source display name; the persisted envelope on its own can render the
    // card in degraded mode (provider treated as "configured" optimistically).
    let oauthEventByToolCallId = $state<Record<string, OAuthRequiredEvent>>({})
    let oauthBlockerActive = $state(data.pendingOAuth !== null)

    function oauthRequiredFromEvent(event: OAuthRequiredEvent): OAuthRequired {
        return {
            approvalId: event.approval_id,
            chatId: data.chat.id,
            sourceId: event.source_id,
            sourceType: event.source_type,
            sourceDisplayName:
                event.source_display_name ??
                getSourceDisplayName(event.source_type as SourceType) ??
                event.source_type,
            provider: event.provider,
            providerConfigured: event.provider_configured ?? true,
            oauthStartUrl: event.oauth_start_url,
            status: 'pending',
        }
    }
    let editingMessageId = $state<string | null>(null)
    let editingContent = $state('')
    // Tracks user's branch choices: parentId -> chosen childId
    let branchSelections = $state<Record<string, string>>({})
    let activeStreamingMessageId = $state<string | null>(null)
    let isAwayFromBottom = $state(false)
    let showTopShadow = $state(false)
    let bottomPadding = $state(80)

    let processedMessages = $derived(processMessages(chatMessages))
    let lastUserMessageIndex = $derived(processedMessages.findLastIndex((m) => m.role === 'user'))
    let drawerOpenByKey = $state<Record<string, boolean>>({})

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
        // Keep the stream open until omni-ai emits end_of_stream for the partial
        // assistant message it persisted on cancellation.
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

    function collectSources(message: ProcessedMessage): NormalizedCitation[] {
        const citations: NormalizedCitation[] = []
        const sourceSet = new Set<string>()
        for (const block of message.content) {
            if (block.type === 'text' && block.citations) {
                for (const citation of block.citations) {
                    const sourceId = sourceIdentityFromCitation(citation)
                    if (!sourceSet.has(sourceId)) {
                        sourceSet.add(sourceId)
                        citations.push(normalizeCitation(citation))
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

        const visited = new Set<string>()
        while (current) {
            if (visited.has(current.id)) {
                console.error('getDisplayPath: message tree cycle detected at', current.id)
                break
            }
            visited.add(current.id)
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
            if (response.status === 409) {
                void resumeActiveStreamIfNeeded()
                toast.info('The previous response is still in progress. Reconnecting to it now.')
            } else {
                console.error('Failed to edit message')
            }
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

        // Select the new branch. Pending interventions belong to the abandoned
        // branch below the edited message, so clear their local UI state before
        // starting the replay stream for this branch.
        branchSelections[parentKey] = messageId
        clearDownstreamSelections(messageId)
        pendingApproval = null
        oauthEventByToolCallId = {}
        oauthBlockerActive = false

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
            updateToolBlock(actionResult.toolUseId, (block) => ({
                ...block,
                actionResult,
                oauthRequired: undefined,
            }))
        }

        const updateOAuthRequired = (toolUseId: string, oauthRequired: OAuthRequired) => {
            updateToolBlock(toolUseId, (block) => ({ ...block, oauthRequired }))
        }

        const updateApprovalRequired = (toolUseId: string, approvalId: string) => {
            updateToolBlock(toolUseId, (block) => ({
                ...block,
                approval: { approvalId, status: ToolApprovalStatus.Pending },
            }))
        }

        for (let i = 0; i < displayPath.length; i++) {
            const chatMsg = displayPath[i]
            const message = chatMsg.message
            const info = siblingInfo.get(chatMsg.id)

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
                            citationTxt += `{omni-cit:${encodeURIComponent(citationIdFromCitation(citation))}}`
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

                            const liveOAuth = oauthEventByToolCallId[block.id]
                            if (liveOAuth) {
                                toolMsgContent.oauthRequired = oauthRequiredFromEvent(liveOAuth)
                            } else if (
                                data.pendingOAuth &&
                                data.pendingOAuth.toolCallId === block.id &&
                                data.pendingOAuth.sourceId &&
                                data.pendingOAuth.sourceType &&
                                data.pendingOAuth.provider &&
                                data.pendingOAuth.oauthStartUrl
                            ) {
                                toolMsgContent.oauthRequired = {
                                    approvalId: data.pendingOAuth.id,
                                    chatId: data.chat.id,
                                    sourceId: data.pendingOAuth.sourceId,
                                    sourceType: data.pendingOAuth.sourceType,
                                    sourceDisplayName:
                                        getSourceDisplayName(
                                            data.pendingOAuth.sourceType as SourceType,
                                        ) ?? data.pendingOAuth.sourceType,
                                    provider: data.pendingOAuth.provider,
                                    providerConfigured: true,
                                    oauthStartUrl: data.pendingOAuth.oauthStartUrl,
                                    status: 'pending',
                                }
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
                            // First text block may carry an Omni envelope (OAuth- or
                            // approval-required prompt). Surface it as a typed UI
                            // variant; if it doesn't parse, fall through to the normal
                            // action-result path.
                            let promptHandled = false
                            if (textBlocks.length > 0) {
                                const envelope = tryParseOmniEnvelope(textBlocks[0].text)
                                if (
                                    envelope &&
                                    envelope.omni_kind === OmniToolResultKind.OauthRequired
                                ) {
                                    const live = oauthEventByToolCallId[toolUseId]
                                    const persistedApprovalId =
                                        data.pendingOAuth?.toolCallId === toolUseId
                                            ? data.pendingOAuth.id
                                            : null
                                    const approvalId = live?.approval_id ?? persistedApprovalId
                                    if (live) {
                                        updateOAuthRequired(toolUseId, oauthRequiredFromEvent(live))
                                        promptHandled = true
                                    } else if (approvalId) {
                                        updateOAuthRequired(toolUseId, {
                                            approvalId,
                                            chatId: data.chat.id,
                                            sourceId: envelope.payload.source_id,
                                            sourceType: envelope.payload.source_type,
                                            sourceDisplayName:
                                                getSourceDisplayName(
                                                    envelope.payload.source_type as SourceType,
                                                ) ?? envelope.payload.source_type,
                                            provider: envelope.payload.provider,
                                            // Optimistic default on refresh; corrected by
                                            // the live SSE event when present, and the
                                            // card itself can re-check via /api/oauth/provider-status.
                                            providerConfigured: true,
                                            oauthStartUrl: envelope.payload.oauth_start_url,
                                            status: 'pending',
                                        })
                                        promptHandled = true
                                    }
                                } else if (
                                    envelope &&
                                    envelope.omni_kind === OmniToolResultKind.ApprovalRequired
                                ) {
                                    updateApprovalRequired(toolUseId, envelope.payload.approval_id)
                                    promptHandled = true
                                }
                            }
                            if (!promptHandled && textBlocks.length > 0) {
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

    function updateScrollState() {
        if (!chatContainerRef) return
        const { scrollTop, scrollHeight, clientHeight } = chatContainerRef
        isAwayFromBottom = scrollTop + clientHeight < scrollHeight - 100
        showTopShadow = scrollTop > 0
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
        if ((page.state as any).stream || data.approvedOAuth) {
            streamResponse(data.chat.id)
        } else {
            void resumeActiveStreamIfNeeded()
        }

        const handleScroll = () => updateScrollState()
        chatContainerRef?.addEventListener('scroll', handleScroll)
        updateScrollState()

        const resizeObserver = new ResizeObserver(() => {
            recalcBottomPadding()
            updateScrollState()
        })
        if (chatContentRef) resizeObserver.observe(chatContentRef)

        // When returning to a backgrounded/frozen tab, the SSE connection is
        // often dead without an 'error' event firing. If we're still streaming,
        // reconnect from the last offset and resume.
        const handleVisibility = () => {
            if (document.visibilityState !== 'visible') return
            if (!isStreaming || activeStreamChatId !== data.chat.id) return
            if (reconnectTimer) return
            // The socket is usually dead on return without ever firing 'error'
            // (it can even still report readyState OPEN). Reconnect if it isn't
            // open or no event/heartbeat arrived within the stall window.
            const stalled = Date.now() - lastStreamEventAt > STREAM_STALL_MS
            const notOpen = !eventSource || eventSource.readyState !== EventSource.OPEN
            if (stalled || notOpen) reconnectStream?.()
        }
        document.addEventListener('visibilitychange', handleVisibility)

        return () => {
            chatContainerRef?.removeEventListener('scroll', handleScroll)
            resizeObserver.disconnect()
            document.removeEventListener('visibilitychange', handleVisibility)
        }
    })

    function streamResponse(chatId: string) {
        const streamRunId = ++streamRunCounter
        const isCurrentStream = () =>
            streamRunId === streamRunCounter && activeStreamChatId === chatId

        isStreaming = true
        activeStreamChatId = chatId
        activeStreamingMessageId = null
        error = null
        errorDetail = null
        startThinkingText()
        oauthBlockerActive = false

        const toolUseStateByIndex = new Map<
            number,
            { id: string; name: string; inputJson: string }
        >()
        let streamCompleted = false
        let messageEventsReceived = 0
        let pauseEventReceived = false
        reconnectAttempts = 0

        const missingStreamMessageId = (reason: string) => {
            console.error('Missing persisted stream message id', { reason })
            error = 'Response stream did not provide a message id. Please reload the chat.'
            errorDetail = null
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
            let targetMessageId = activeStreamingMessageId
            let targetMessageIndex = targetMessageId
                ? chatMessages.findIndex((message) => message.id === targetMessageId)
                : -1
            let lastMessage =
                targetMessageIndex === -1
                    ? chatMessages[chatMessages.length - 1]
                    : chatMessages[targetMessageIndex]

            if (
                block.type !== 'tool_result' &&
                (!lastMessage ||
                    lastMessage.message.role !== 'assistant' ||
                    !Array.isArray(lastMessage.message.content))
            ) {
                console.error('Received streamed assistant content before message_start', {
                    blockType: block.type,
                    blockIdx,
                    activeStreamingMessageId,
                })
                error = 'Response stream sent content before a message id. Please reload the chat.'
                errorDetail = null
                return
            }

            if (!lastMessage) {
                console.error('No last message found when streaming response')
                return
            }

            const replaceLastMessage = (message: ChatMessage) => {
                const replaceIndex =
                    targetMessageIndex === -1 ? chatMessages.length - 1 : targetMessageIndex
                chatMessages = [
                    ...chatMessages.slice(0, replaceIndex),
                    message,
                    ...chatMessages.slice(replaceIndex + 1),
                ]
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
                    const messageId = (block as ToolResultBlockParam & { message_id?: string })
                        .message_id
                    if (!messageId) {
                        missingStreamMessageId('tool-result-message')
                        return
                    }
                    const displayPath = getDisplayPath(chatMessages)
                    const toolParentId =
                        displayPath.length > 0 ? displayPath[displayPath.length - 1].id : undefined
                    const toolResultMessage: ChatMessage = {
                        id: messageId,
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

            // Each (re)connect gets a fresh stall window so the watchdog below
            // doesn't fire while a new connection is still being established.
            lastStreamEventAt = Date.now()

            eventSource.addEventListener('message_id', (event) => {
                if (!isCurrentStream()) return
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
            })

            eventSource.addEventListener('not_resumable', () => {
                if (!isCurrentStream()) return
                // The buffered run is gone (expired or never existed). Reload the
                // persisted messages from the server and clear streaming state.
                streamCompleted = true
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                stopThinkingText()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()
                invalidateAll()
            })

            eventSource.addEventListener('title', () => {
                if (!isCurrentStream()) return
                void invalidate('app:recent_chats')
                void invalidate(`app:chat:${chatId}`)
            })

            eventSource.addEventListener('heartbeat', () => {
                if (!isCurrentStream()) return
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
            })

            eventSource.addEventListener('compaction_start', (event) => {
                if (!isCurrentStream()) return
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
                stopThinkingText()
                isCompacting = true
                thinkingText = 'Summarizing conversation'
                updateScrollState()
            })

            eventSource.addEventListener('compaction_end', (event) => {
                if (!isCurrentStream()) return
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
                if (isCompacting) startThinkingText()
                updateScrollState()
            })

            eventSource.addEventListener('message', (event) => {
                if (!isCurrentStream()) return
                streamLastEventId = event.lastEventId || streamLastEventId
                lastStreamEventAt = Date.now()
                reconnectAttempts = 0
                try {
                    const data: MessageStreamEvent | ToolResultBlockParam = JSON.parse(event.data)
                    if (data.type === 'message_start') {
                        if (isCompacting) startThinkingText()
                        const messageId = data.message.id
                        if (!messageId) {
                            missingStreamMessageId('message_start')
                            return
                        }
                        // On a reload/reconnect replay, the assistant row was already
                        // persisted at the original message_start and loaded from the DB,
                        // so it is already in chatMessages. Re-appending it would derive
                        // its parent from the display-path leaf (itself) and create a
                        // self-cycle (id === parentId) that hangs getDisplayPath. Adopt
                        // the existing row as the streaming target and let the replayed
                        // content deltas rebuild it in place instead.
                        const existingIndex = chatMessages.findIndex((m) => m.id === messageId)
                        if (existingIndex !== -1) {
                            const existing = chatMessages[existingIndex]
                            chatMessages = [
                                ...chatMessages.slice(0, existingIndex),
                                {
                                    ...existing,
                                    message: {
                                        role: data.message.role,
                                        content: data.message.content,
                                    },
                                },
                                ...chatMessages.slice(existingIndex + 1),
                            ]
                            activeStreamingMessageId = messageId
                            return
                        }
                        // Find the last message in current display path to use as parent
                        const displayPath = getDisplayPath(chatMessages)
                        const streamParentId =
                            displayPath.length > 0
                                ? displayPath[displayPath.length - 1].id
                                : undefined
                        const startedMessage: ChatMessage = {
                            id: messageId,
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

                    updateScrollState()
                } catch (err) {
                    console.error('Failed to parse SSE data:', event.data, err)
                } finally {
                    messageEventsReceived += 1
                }
            })

            eventSource.addEventListener('approval_required', (event) => {
                if (!isCurrentStream()) return
                pauseEventReceived = true
                try {
                    const approvalData: ApprovalRequiredEvent = JSON.parse(event.data)
                    pendingApproval = approvalData
                    isStreaming = false
                    stopInProgress = false
                    activeStreamingMessageId = null
                    stopThinkingText()
                    requestAnimationFrame(() => recalcBottomPadding())
                } catch (err) {
                    console.error('Failed to parse approval_required event:', err)
                }
            })

            eventSource.addEventListener('oauth_required', (event) => {
                if (!isCurrentStream()) return
                pauseEventReceived = true
                try {
                    const oauthData: OAuthRequiredEvent = JSON.parse(event.data)
                    oauthEventByToolCallId = {
                        ...oauthEventByToolCallId,
                        [oauthData.tool_call_id]: oauthData,
                    }
                    oauthBlockerActive = true
                    pendingApproval = null
                    isStreaming = false
                    stopInProgress = false
                    activeStreamingMessageId = null
                    stopThinkingText()
                    requestAnimationFrame(() => recalcBottomPadding())
                } catch (err) {
                    console.error('Failed to parse oauth_required event:', err)
                }
            })

            eventSource.addEventListener('end_of_stream', () => {
                if (!isCurrentStream()) return
                const wasStopping = stopInProgress
                streamCompleted = true
                isStreaming = false
                stopInProgress = false
                activeStreamingMessageId = null
                stopThinkingText()
                requestAnimationFrame(() => recalcBottomPadding())
                userInputRef?.focus()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()

                if (messageEventsReceived === 0 && !pauseEventReceived && !error && !wasStopping) {
                    error = 'Failed to generate response. Please try again.'
                }
            })

            const handleStreamError = (event: Event) => {
                if (!isCurrentStream()) return
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
                stopThinkingText()
                requestAnimationFrame(() => recalcBottomPadding())
                userInputRef?.focus()
                eventSource?.close()
                eventSource = null
                activeStreamChatId = null
                clearReconnectState()
            }

            const handleConnectionError = () => {
                if (!isCurrentStream()) return
                // Guard against treating an intentional stop() as a connection error.
                // handleStop() sets isStreaming = false before the EventSource fires its
                // error event, so we can use that as a signal to skip cleanup here.
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
        // after a freeze or half-open connection: if the stream goes silent, force
        // a reconnect from the last offset. The server emits heartbeat events while
        // the agent is legitimately idle, so a healthy long-running tool call will
        // not trip this timer.
        if (streamWatchdog) clearInterval(streamWatchdog)
        streamWatchdog = setInterval(() => {
            if (!isStreaming || streamCompleted || activeStreamChatId !== chatId) return
            if (reconnectTimer) return
            // Heartbeats arrive ~every 15s while the run is alive, so no event within
            // the stall window means the socket is dead — even if the browser
            // still reports it OPEN (the half-open socket left by a backgrounded
            // tab). Reconnect from the last offset regardless of readyState.
            if (Date.now() - lastStreamEventAt > STREAM_STALL_MS) reconnectStream?.()
        }, 5000)
    }

    async function handleApproval(decision: 'approved' | 'denied') {
        console.log('Tool approval decision requested', { decision, pendingApproval })
        if (!pendingApproval) {
            console.warn('Tool approval decision ignored because no pending approval is set', {
                decision,
            })
            return
        }

        const approvalIds = pendingApproval.approvals?.map((approval) => approval.approval_id) ?? [
            pendingApproval.approval_id,
        ]
        const approvalId = approvalIds[0]

        try {
            console.log('Submitting tool approval decision', {
                decision,
                approvalId,
                approvalIds,
                chatId: data.chat.id,
            })
            const response = await fetch(`/api/chat/${data.chat.id}/approve`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    approvalId,
                    approvalIds,
                    decision,
                }),
            })
            console.log('Tool approval decision response received', {
                decision,
                approvalId,
                approvalIds,
                status: response.status,
                ok: response.ok,
            })

            if (!response.ok) {
                const responseText = await response.text().catch(() => null)
                console.error('Failed to submit approval decision', {
                    decision,
                    approvalId,
                    approvalIds,
                    status: response.status,
                    responseText,
                })
                return
            }

            const responseBody = (await response.json()) as { denialMessageId?: string | null }
            const approvalItems = pendingApproval.approvals ?? [pendingApproval]
            if (decision === 'denied' && responseBody.denialMessageId) {
                const firstToolCallId = approvalItems[0]?.tool_call_id
                const parentMessage = chatMessages.find((message) => {
                    const content = message.message.content
                    return (
                        Array.isArray(content) &&
                        content.some(
                            (block) => block.type === 'tool_use' && block.id === firstToolCallId,
                        )
                    )
                })
                if (parentMessage) {
                    const denialBlocks: ToolResultBlockParam[] = approvalItems.map((approval) => ({
                        type: 'tool_result',
                        tool_use_id: approval.tool_call_id,
                        content: [
                            {
                                type: 'text',
                                text: 'The user denied approval for this tool call.',
                            },
                        ],
                        is_error: true,
                    }))
                    const denialMessage: ChatMessage = {
                        id: responseBody.denialMessageId,
                        chatId: data.chat.id,
                        parentId: parentMessage.id,
                        message: {
                            role: 'user',
                            content: denialBlocks,
                        },
                        contentText: null,
                        messageSeqNum: nextMessageSeqNum(chatMessages),
                        createdAt: new Date(),
                    }
                    chatMessages = [...chatMessages, denialMessage]
                    selectBranch(parentMessage.id, denialMessage.id)
                }
            }

            pendingApproval = null
            console.log('Cleared pending tool approval', { decision, approvalId, approvalIds })

            if (decision === 'approved') {
                console.log('Resuming stream after approval', {
                    approvalId,
                    approvalIds,
                    chatId: data.chat.id,
                })
                // Re-trigger stream to resume execution
                streamResponse(data.chat.id)
            }
        } catch (err) {
            console.error('Error submitting approval:', err, { decision, approvalId })
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
            if (response.status === 409) {
                void resumeActiveStreamIfNeeded()
                toast.info('The previous response is still in progress. Reconnecting to it now.')
            } else {
                console.error('Failed to send message to chat session')
            }
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

        pendingUploads = []
        isAwayFromBottom = false

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
                        aria-label="Retry message"
                        data-testid={`retry-message-${message.origMessageId}`}
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

{#snippet sourcesDrawer(citations: NormalizedCitation[], drawerKey: string)}
    <Drawer.Root
        open={drawerOpenByKey[drawerKey] ?? false}
        direction="right"
        autoFocus={true}
        onOpenChange={(o) => {
            drawerOpenByKey = { ...drawerOpenByKey, [drawerKey]: o }
        }}>
        <Drawer.Trigger>
            {#snippet child({ props })}
                <Button
                    {...props}
                    size="sm"
                    variant="outline"
                    class="cursor-pointer gap-1.5 text-xs">
                    <BookOpen class="h-3.5 w-3.5" />
                    {citations.length} source{citations.length > 1 ? 's' : ''}
                </Button>
            {/snippet}
        </Drawer.Trigger>
        <Drawer.Content
            class="m-2 flex max-h-full flex-col overflow-hidden rounded-xl border shadow-lg">
            <Drawer.Header class="relative px-5 py-3 text-left">
                <Drawer.Title class="text-base font-semibold">Sources</Drawer.Title>
                <Drawer.Description class="sr-only">
                    Cited sources for this response
                </Drawer.Description>
                <Drawer.Close
                    class="ring-offset-background focus-visible:ring-ring absolute top-3 right-4 cursor-pointer rounded-xs opacity-70 transition-opacity hover:opacity-100 focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-hidden disabled:pointer-events-none">
                    <X class="size-4" />
                    <span class="sr-only">Close</span>
                </Drawer.Close>
            </Drawer.Header>
            <div class="min-h-0 flex-1 overflow-y-auto px-3 pt-3 pb-5">
                <div class="flex flex-col gap-2">
                    {#each citations as citation (citation.sourceId)}
                        <svelte:element
                            this={citation.href ? 'a' : 'div'}
                            href={citation.href ?? undefined}
                            data-testid="drawer-source"
                            class="hover:bg-muted/40 rounded-lg p-2 px-2.5 text-xs font-normal no-underline transition-colors"
                            target={citation.href ? '_blank' : undefined}
                            rel={citation.href ? 'noopener noreferrer' : undefined}>
                            <div class="flex items-center gap-2">
                                {#if citation.isImap}
                                    <Mail class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {:else if citation.iconHint && getIconFromSearchResult(citation.iconHint)}
                                    <img
                                        src={getIconFromSearchResult(citation.iconHint)}
                                        alt=""
                                        class="!m-0 h-4 w-4 flex-shrink-0" />
                                {:else if citation.sourceName === 'Web'}
                                    <Globe class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {:else}
                                    <FileText class="text-muted-foreground h-4 w-4 flex-shrink-0" />
                                {/if}
                                <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                                    <p
                                        data-testid="drawer-source-title"
                                        class="text-muted-foreground text-sm font-semibold">
                                        {citation.title}
                                    </p>
                                    <p class="text-muted-foreground/60 text-xs">
                                        {citation.sourceName}
                                    </p>
                                    {#if citation.isImap}
                                        <ImapCitationSource
                                            source={citation.iconHint ?? undefined} />
                                    {:else if citation.locationLabel}
                                        <p class="text-muted-foreground/70 text-xs">
                                            {citation.locationLabel}
                                        </p>
                                    {/if}
                                </div>
                                {#if citation.href}
                                    <ExternalLink
                                        class="text-muted-foreground h-3 w-3 flex-shrink-0" />
                                {/if}
                            </div>
                        </svelte:element>
                    {/each}
                </div>
            </div>
        </Drawer.Content>
    </Drawer.Root>
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
                                class="prose prose-sm md:prose-base prose-p:my-3 prose-headings:text-foreground prose-p:text-foreground prose-li:text-foreground prose-strong:text-foreground prose-code:text-foreground prose-a:text-primary dark:prose-invert max-w-none min-w-0 overflow-visible">
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
                            {#if pendingApproval && !oauthBlockerActive && i === processedMessages.length - 1}
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

                                    <!-- Action summary -->
                                    <Card.Content class="px-5 py-4">
                                        {@const approvalItems = pendingApproval.approvals ?? [
                                            pendingApproval,
                                        ]}
                                        {@const approvalFields = approvalInputDisplayFields(
                                            pendingApproval.tool_input,
                                        )}
                                        <div class="space-y-4">
                                            <div class="space-y-1">
                                                <div class="text-sm font-medium">
                                                    {#if approvalItems.length === 1}
                                                        Omni wants to {approvalActionLabel(
                                                            pendingApproval.tool_name,
                                                        )}.
                                                    {:else}
                                                        Omni wants to perform {approvalItems.length}
                                                        actions.
                                                    {/if}
                                                </div>
                                                <p class="text-muted-foreground text-xs">
                                                    Review the important details below before
                                                    approving {approvalItems.length === 1
                                                        ? 'this action'
                                                        : 'these actions'}.
                                                </p>
                                            </div>

                                            {#if approvalItems.length > 1}
                                                <div class="space-y-1 text-[13px]">
                                                    {#each approvalItems as approval}
                                                        <div>
                                                            • {approvalActionLabel(
                                                                approval.tool_name,
                                                            )}
                                                        </div>
                                                    {/each}
                                                </div>
                                            {/if}

                                            {#if approvalFields.length > 0}
                                                <div class="space-y-2 text-[13px]">
                                                    {#each approvalFields as field}
                                                        <div
                                                            class="grid grid-cols-[120px_1fr] items-start gap-x-4 gap-y-2">
                                                            <div class="text-muted-foreground">
                                                                {field.label}
                                                            </div>
                                                            <div
                                                                class="break-words whitespace-pre-wrap">
                                                                {field.value}
                                                            </div>
                                                        </div>
                                                    {/each}
                                                </div>
                                            {/if}

                                            <details class="text-muted-foreground text-xs">
                                                <summary class="cursor-pointer select-none">
                                                    Show technical details
                                                </summary>
                                                <pre
                                                    class="bg-muted/60 mt-2 max-h-48 overflow-auto rounded-md p-3 text-[11px] whitespace-pre-wrap">{formatApprovalTechnicalDetails(
                                                        pendingApproval.tool_input,
                                                    )}</pre>
                                            </details>
                                        </div>
                                    </Card.Content>

                                    <!-- Footer -->
                                    <Card.Footer
                                        class="bg-muted/50 justify-end gap-2 border-t px-3 py-3 [.border-t]:py-3">
                                        <Button
                                            size="sm"
                                            variant="outline"
                                            class="cursor-pointer"
                                            onclick={() => {
                                                console.log('Tool approval deny button clicked', {
                                                    pendingApproval,
                                                })
                                                handleApproval('denied')
                                            }}>
                                            Deny
                                        </Button>
                                        <Button
                                            size="sm"
                                            variant="default"
                                            class="cursor-pointer"
                                            onclick={() => {
                                                console.log(
                                                    'Tool approval approve button clicked',
                                                    {
                                                        pendingApproval,
                                                    },
                                                )
                                                handleApproval('approved')
                                            }}>
                                            <Check class="h-3 w-3" />
                                            Approve & send
                                        </Button>
                                    </Card.Footer>
                                </Card.Root>
                            {/if}
                            {#if !isStreaming}
                                {@const citations = collectSources(message)}
                                {#if citations.length > 0}
                                    <div class="flex items-center gap-2">
                                        {@render sourcesDrawer(citations, message.renderKey)}
                                    </div>
                                {/if}
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

                {#if pendingApproval && !oauthBlockerActive && processedMessages[processedMessages.length - 1]?.role !== 'assistant'}
                    {@const connectorName = pendingApproval.tool_name.split('__')[0]}
                    {@const actionName = pendingApproval.tool_name.split('__').slice(1).join('__')}
                    {@const connectorIcon = getSourceIconPath(connectorName)}
                    <Card.Root class="gap-0 overflow-hidden py-0">
                        <Card.Header
                            class="flex items-center gap-3 border-b px-5 py-3 [.border-b]:py-3">
                            {#if connectorIcon}
                                <img src={connectorIcon} alt={connectorName} class="h-7 w-7" />
                            {/if}
                            <div class="min-w-0 flex-1">
                                <Card.Title class="text-sm">
                                    {getSourceDisplayName(connectorName as SourceType) ||
                                        connectorName}
                                </Card.Title>
                                <Card.Description class="text-xs">
                                    {actionName.replaceAll('_', ' ')}
                                </Card.Description>
                            </div>
                            <div
                                class="flex items-center gap-1.5 rounded-full bg-amber-100 px-2.5 py-1 dark:bg-amber-950">
                                <span class="h-1.5 w-1.5 rounded-full bg-amber-500"></span>
                                <span
                                    class="text-[11px] font-medium text-amber-700 dark:text-amber-400"
                                    >Awaiting approval</span>
                            </div>
                        </Card.Header>
                        <Card.Content class="px-5 py-4">
                            {@const approvalItems = pendingApproval.approvals ?? [pendingApproval]}
                            {@const approvalFields = approvalInputDisplayFields(
                                pendingApproval.tool_input,
                            )}
                            <div class="space-y-4">
                                <div class="space-y-1">
                                    <div class="text-sm font-medium">
                                        {#if approvalItems.length === 1}
                                            Omni wants to {approvalActionLabel(
                                                pendingApproval.tool_name,
                                            )}.
                                        {:else}
                                            Omni wants to perform {approvalItems.length} actions.
                                        {/if}
                                    </div>
                                    <p class="text-muted-foreground text-xs">
                                        Review the important details below before approving {approvalItems.length ===
                                        1
                                            ? 'this action'
                                            : 'these actions'}.
                                    </p>
                                </div>

                                {#if approvalItems.length > 1}
                                    <div class="space-y-1 text-[13px]">
                                        {#each approvalItems as approval}
                                            <div>• {approvalActionLabel(approval.tool_name)}</div>
                                        {/each}
                                    </div>
                                {/if}

                                {#if approvalFields.length > 0}
                                    <div class="space-y-2 text-[13px]">
                                        {#each approvalFields as field}
                                            <div
                                                class="grid grid-cols-[120px_1fr] items-start gap-x-4 gap-y-2">
                                                <div class="text-muted-foreground">
                                                    {field.label}
                                                </div>
                                                <div class="break-words whitespace-pre-wrap">
                                                    {field.value}
                                                </div>
                                            </div>
                                        {/each}
                                    </div>
                                {/if}
                            </div>
                        </Card.Content>
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
                {#if isAwayFromBottom}
                    <div
                        class="pointer-events-none absolute inset-x-0 -top-12 z-20 mx-auto flex w-full max-w-4xl justify-center px-4">
                        <Button
                            type="button"
                            variant="outline"
                            size="icon"
                            class="bg-card pointer-events-auto h-9 w-9 cursor-pointer rounded-full shadow-md"
                            aria-label="Scroll to bottom"
                            title="Scroll to bottom"
                            onclick={scrollToBottom}>
                            <ArrowDown class="h-4 w-4" />
                        </Button>
                    </div>
                {/if}
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
                        {isStreaming}
                        {stopInProgress}
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
