<script lang="ts">
    import * as Accordion from '$lib/components/ui/accordion'
    import { Button } from '$lib/components/ui/button/index.js'
    import { Input } from '$lib/components/ui/input/index.js'
    import type {
        MessageParam,
        MessageStreamEvent,
        SearchResultBlockParam,
        TextBlockParam,
        ToolUseBlock,
        TextDelta,
        InputJSONDelta,
    } from '@anthropic-ai/sdk/resources/messages'
    import { Search, Send, Sparkles } from '@lucide/svelte'
    import { marked } from 'marked'
    import { onMount } from 'svelte'
    import type { PageProps } from './$types.js'
    import type {
        ProcessedMessage,
        TextMessageContent,
        ToolMessageContent,
        MessageContent,
    } from '$lib/types/message.js'
    import ToolMessage from '$lib/components/tool-message.svelte'
    import { cn } from '$lib/utils.js'

    let { data }: PageProps = $props()

    let userMessage = $state('')

    let isLoading = $state(false)
    let answer = $state('')
    let error = $state<string | null>(null)

    let processedMessages = $derived(processMessages(data.messages.map((m) => m.message)))
    let streamingResponseMessage = $state<ProcessedMessage | null>(null)

    $inspect(processedMessages).with((t, v) => {
        console.log('Processed Messages:', v)
        for (const m of processedMessages) {
            if (m.role === 'user' && m.content.length > 1) {
                console.error('User message has more than one content block:', m)
            }
        }
    })

    function processMessages(messages: MessageParam[]): ProcessedMessage[] {
        const processedMessages: ProcessedMessage[] = []

        const addMessage = (message: ProcessedMessage) => {
            const lastMessage = processedMessages[processedMessages.length - 1]
            let messageToUpdate: ProcessedMessage
            if (!lastMessage || lastMessage.role !== message.role) {
                const newMessage = {
                    id: processedMessages.length,
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
                    lastBlock.text += '\n' + block.text
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
                    role: 'user',
                    content: userMessageContent,
                }

                addMessage(processedUserMessage)
            } else {
                // Here we handle both assistant messages (with possible tool uses) and also user messages that contain tool results
                const processedMessage: ProcessedMessage = {
                    id: processedMessages.length,
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

    // Start streaming when we have a query
    onMount(() => {
        streamAIResponse(data.chat.id)
    })

    function streamAIResponse(chatId: string) {
        isLoading = true
        answer = ''
        error = null

        let currToolUseId: string
        let currToolUseInputStr: string

        const eventSource = new EventSource(`/api/chat/${chatId}/stream`, { withCredentials: true })

        let accumulatedText = ''
        let streamCompleted = false

        streamingResponseMessage = {
            id: processedMessages.length,
            role: 'assistant',
            content: [],
        }

        const updateStreamingResponse = (block: ToolUseBlock | TextDelta | InputJSONDelta) => {
            if (!streamingResponseMessage) {
                streamingResponseMessage = {
                    id: processedMessages.length,
                    role: 'assistant',
                    content: [],
                }
            }

            const lastBlock =
                streamingResponseMessage.content[streamingResponseMessage.content.length - 1]
            if (block.type === 'text_delta') {
                if (lastBlock && lastBlock.type === 'text') {
                    // Combine text blocks
                    lastBlock.text += block.text
                } else {
                    streamingResponseMessage.content.push({
                        id: streamingResponseMessage.content.length,
                        type: 'text',
                        text: block.text,
                    })
                }
            } else if (block.type === 'tool_use') {
                const existingToolUse = streamingResponseMessage.content.find(
                    (b) => b.type === 'tool' && b.toolUse.id === block.id,
                )
                if (existingToolUse) {
                    ;(existingToolUse as ToolMessageContent).toolUse.input = block.input
                } else {
                    streamingResponseMessage.content.push({
                        id: streamingResponseMessage.content.length,
                        type: 'tool',
                        toolUse: {
                            id: block.id,
                            name: block.name,
                            input: block.input,
                        },
                    })
                }
            }
        }

        eventSource.addEventListener('message', (event) => {
            try {
                const data: MessageStreamEvent = JSON.parse(event.data)
                if (data.type === 'content_block_start') {
                    if (
                        data.content_block.type === 'tool_use' &&
                        data.content_block.name === 'search_documents'
                    ) {
                        accumulatedText += '\n\n🔍 Searching documents...\n\n'
                        answer = accumulatedText
                        updateStreamingResponse(data.content_block)
                        isLoading = false
                        currToolUseId = data.content_block.id
                        currToolUseInputStr = ''
                    }
                } else if (data.type === 'content_block_delta') {
                    if (data.delta.type === 'text_delta' && data.delta.text) {
                        accumulatedText += data.delta.text
                        answer = accumulatedText
                        updateStreamingResponse(data.delta)
                        isLoading = false
                    } else if (data.delta.type === 'input_json_delta') {
                        // Parse partial JSON to show search query if possible
                        currToolUseInputStr += data.delta.partial_json
                        try {
                            const parsedInput = JSON.parse(currToolUseInputStr)
                            updateStreamingResponse({
                                type: 'tool_use',
                                id: currToolUseId,
                                name: 'search_documents',
                                input: parsedInput,
                            })
                        } catch (err) {
                            // Ignore JSON parse errors for partial input
                        }
                    }
                }
            } catch (err) {
                console.warn('Failed to parse SSE data:', event.data, err)
            }
        })

        eventSource.addEventListener('end_of_stream', () => {
            streamCompleted = true
            isLoading = false
            eventSource.close()
        })

        eventSource.addEventListener('error', (event) => {
            error = 'Failed to generate response. Please try again.'
            isLoading = false
            eventSource.close()
        })
    }

    function handleSubmit() {
        if (userMessage.trim()) {
        }
    }

    function handleKeyPress(event: KeyboardEvent) {
        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault()
            handleSubmit()
        }
    }
</script>

<svelte:head>
    <title>Omni Chat</title>
</svelte:head>

<div class="flex h-full flex-col">
    <!-- Chat Container -->
    <div class="flex-1 overflow-y-auto px-4 pt-6">
        <div class="mx-auto mb-20 max-w-4xl">
            <!-- Existing Messages -->
            {#each processedMessages as message (message.id)}
                {#if message.role === 'user'}
                    <!-- User Message -->
                    <div class="mb-6 flex">
                        <div class="text-foreground max-w-[80%] rounded-2xl bg-gray-100 px-6 py-4">
                            {@html marked.parse((message.content[0] as TextMessageContent).text)}
                        </div>
                    </div>
                {:else if message.role === 'assistant'}
                    <!-- Assistant Message -->
                    <div class="mb-6">
                        <div class="flex items-start gap-3">
                            <div
                                class="flex h-8 w-8 items-center justify-center rounded-full bg-gradient-to-br from-purple-500 to-pink-500">
                                <Sparkles class="h-5 w-5 text-white" />
                            </div>
                            <div class="flex-1">
                                <div class="prose max-w-none">
                                    {#each message.content as block (block.id)}
                                        {#if block.type === 'text'}
                                            {@html marked.parse(block.text)}
                                        {:else if block.type === 'tool'}
                                            <ToolMessage message={block} />
                                        {/if}
                                    {/each}
                                </div>
                            </div>
                        </div>
                    </div>
                {/if}
            {/each}

            <!-- Streaming AI Response (only show if streaming is active) -->
            {#if isLoading || answer || error}
                <div class="mb-6">
                    <div class="flex items-start gap-3">
                        <div
                            class="flex h-8 w-8 items-center justify-center rounded-full bg-gradient-to-br from-purple-500 to-pink-500">
                            <Sparkles class="h-5 w-5 text-white" />
                        </div>
                        <div class="flex-1">
                            {#if isLoading && !answer}
                                <div class="text-gray-500">Thinking...</div>
                            {:else if error}
                                <div class="text-red-600">
                                    {error}
                                </div>
                            {:else if streamingResponseMessage}
                                <div class="prose max-w-none">
                                    {#each streamingResponseMessage.content as block (block.id)}
                                        {#if block.type === 'text'}
                                            {@html marked.parse(block.text)}
                                        {:else if block.type === 'tool'}
                                            <ToolMessage message={block} />
                                        {/if}
                                    {/each}
                                </div>
                            {/if}
                        </div>
                    </div>
                </div>
            {/if}
        </div>

        <!-- Input -->
        <div class="bg-background sticky bottom-0 flex justify-center pb-6">
            <div
                class="bg-card flex max-h-96
                min-h-[1.5rem] w-full max-w-4xl flex-col
                gap-2 rounded-xl border border-gray-200 p-4 shadow-xl">
                <div
                    bind:innerText={userMessage}
                    class={cn(
                        'before:text-muted-foreground relative cursor-text overflow-y-auto before:absolute before:inset-0 focus:outline-none',
                        userMessage
                            ? "before:content-['']"
                            : 'before:content-[attr(data-placeholder)]',
                    )}
                    contenteditable="true"
                    role="textbox"
                    aria-multiline="true"
                    data-placeholder={'Ask a follow-up...'}>
                </div>
                <div class="flex w-full justify-end">
                    <Button size="icon">
                        <Send class="h-4 w-4" />
                    </Button>
                </div>
            </div>
        </div>
    </div>
</div>
