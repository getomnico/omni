<script lang="ts">
    import { Button } from '$lib/components/ui/button'
    import type {
        MessageParam,
        MessageStreamEvent,
        SearchResultBlockParam,
        TextBlockParam,
        ToolUseBlock,
        TextDelta,
        InputJSONDelta,
    } from '@anthropic-ai/sdk/resources/messages'
    import { Send, Copy, ThumbsUp, ThumbsDown, Share, Check, CircleStop } from '@lucide/svelte'
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
    import { afterNavigate } from '$app/navigation'

    let { data }: PageProps = $props()
    let chatMessages = $state<ChatMessage[]>([...data.messages])

    afterNavigate(() => {
        chatMessages = [...data.messages]
    })

    let userMessage = $state('')
    let inputRef: HTMLDivElement
    let chatContainerRef: HTMLDivElement
    let lastUserMessageRef: HTMLDivElement | null = $state(null)

    let isStreaming = $state(false)
    let error = $state<string | null>(null)

    let processedMessages = $derived(processMessages(chatMessages))
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
                        processedMessage.content.push({
                            id: processedMessage.content.length,
                            type: 'text',
                            text: block.text,
                        })
                    } else {
                        // Tool use or result
                        if (block.type === 'tool_use') {
                            // Tool use always comes first, so we create the corresponding output block
                            const toolComponentData: ToolMessageContent = {
                                id: 0,
                                type: 'tool',
                                toolUse: {
                                    id: block.id,
                                    name: block.name,
                                    input: block.input,
                                },
                            }

                            processedMessage.content.push(toolComponentData)
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

        const updateStreamingResponse = (
            block: ToolUseBlock | TextDelta | InputJSONDelta | ToolResultBlockParam,
        ) => {
            const lastMessage = chatMessages[chatMessages.length - 1]
            if (!lastMessage) {
                // This should never happen
                console.error('No last message found when streaming response')
                return
            }

            const existingBlocks = lastMessage.message.content as ContentBlockParam[]
            const lastBlock = existingBlocks[existingBlocks.length - 1]
            if (block.type === 'text_delta') {
                if (lastBlock && lastBlock.type === 'text') {
                    // Combine text blocks
                    lastBlock.text += block.text
                } else {
                    existingBlocks.push({
                        type: 'text',
                        text: block.text,
                    })
                }
            } else if (block.type === 'tool_use') {
                const existingToolUse = existingBlocks.find(
                    (b) => b.type === 'tool_use' && b.id === block.id,
                )

                if (existingToolUse) {
                    ;(existingToolUse as ToolUseBlock).input = block.input
                } else {
                    existingBlocks.push({
                        type: 'tool_use',
                        id: block.id,
                        name: block.name,
                        input: block.input,
                    })
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
                        updateStreamingResponse(data.content_block)
                        currToolUseId = data.content_block.id
                        currToolUseName = data.content_block.name
                        currToolUseInputStr = ''
                    }
                } else if (data.type === 'content_block_delta') {
                    if (data.delta.type === 'text_delta' && data.delta.text) {
                        updateStreamingResponse(data.delta)
                    } else if (data.delta.type === 'input_json_delta') {
                        // Parse partial JSON to show search query if possible
                        currToolUseInputStr += data.delta.partial_json
                        try {
                            const parsedInput = JSON.parse(currToolUseInputStr)
                            updateStreamingResponse({
                                type: 'tool_use',
                                id: currToolUseId,
                                name: currToolUseName,
                                input: parsedInput,
                            })
                        } catch (err) {
                            // Ignore JSON parse errors for partial input
                        }
                    }
                } else if (data.type == 'tool_result') {
                    updateStreamingResponse(data)
                }

                scrollToBottom()
            } catch (err) {
                console.warn('Failed to parse SSE data:', event.data, err)
            }
        })

        eventSource.addEventListener('end_of_stream', () => {
            streamCompleted = true
            isStreaming = false
            eventSource.close()
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

    async function handleKeyPress(event: KeyboardEvent) {
        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            await handleSubmit()
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

<div class="flex h-full flex-col">
    <!-- Chat Container -->
    <div bind:this={chatContainerRef} class="flex w-full flex-1 flex-col overflow-y-auto px-4 pt-6">
        <div class="mx-auto mb-20 flex w-full max-w-4xl flex-1 flex-col gap-8">
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
                        <div class="prose max-w-none">
                            {#each message.content as block (block.id)}
                                {#if block.type === 'text'}
                                    {@html marked.parse(block.text)}
                                {:else if block.type === 'tool'}
                                    <div class="mb-1">
                                        <ToolMessage message={block} />
                                    </div>
                                {/if}
                            {/each}
                        </div>
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
                        <div class="text-sm text-red-600">
                            {error}
                        </div>
                    {:else if isStreaming}
                        <span class="mt-2 flex items-center gap-1">
                            <span class="thinking-dot"></span>
                        </span>
                    {/if}
                </div>
            {/if}
        </div>

        <!-- Input -->
        <div class="bg-background sticky bottom-0 flex justify-center pb-2">
            <div
                class="bg-card flex max-h-96
                min-h-[1.5rem] w-full max-w-4xl cursor-text
                flex-col gap-2 rounded-xl border border-gray-200 p-4 shadow-sm"
                onclick={() => inputRef.focus()}
                onkeydown={handleKeyPress}
                role="button"
                tabindex="0">
                <div
                    bind:this={inputRef}
                    bind:innerText={userMessage}
                    class={cn(
                        'before:text-muted-foreground relative cursor-text overflow-y-auto before:absolute before:inset-0 focus:outline-none',
                        userMessage.trim()
                            ? "before:content-['']"
                            : 'before:content-[attr(data-placeholder)]',
                    )}
                    contenteditable="true"
                    role="textbox"
                    aria-multiline="true"
                    data-placeholder="Ask a follow-up...">
                </div>
                <div class="flex w-full justify-end">
                    {#if isStreaming}
                        <Button size="icon" class="cursor-pointer rounded-full" onclick={() => {}}>
                            <CircleStop class="h-4 w-4" />
                        </Button>
                    {:else}
                        <Button
                            size="icon"
                            class="cursor-pointer"
                            onclick={handleSubmit}
                            disabled={!userMessage.trim()}>
                            <Send class="h-4 w-4" />
                        </Button>
                    {/if}
                </div>
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
