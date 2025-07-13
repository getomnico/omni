<script lang="ts">
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Loader2, Bot, ExternalLink } from '@lucide/svelte'
    import type { SearchRequest } from '$lib/types/search.js'
    import { marked } from 'marked'

    interface Props {
        searchRequest: SearchRequest
    }

    let { searchRequest }: Props = $props()

    let isLoading = $state(true)
    let answer = $state('')
    let error = $state<string | null>(null)

    // Start streaming AI answer automatically when component mounts
    $effect(() => {
        if (searchRequest.query.trim()) {
            streamAIAnswer()
        }
    })

    async function streamAIAnswer() {
        isLoading = true
        answer = ''
        error = null

        try {
            const response = await fetch('/api/search/ai-answer', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify(searchRequest),
            })

            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`)
            }

            const reader = response.body?.getReader()
            if (!reader) {
                throw new Error('No response body reader available')
            }

            const decoder = new TextDecoder()

            while (true) {
                const { done, value } = await reader.read()

                if (done) {
                    break
                }

                const chunk = decoder.decode(value, { stream: true })
                answer += chunk
                isLoading = false
            }
        } catch (err) {
            console.error('Error streaming AI answer:', err)
            error = 'Failed to generate AI answer. Please try again.'
            isLoading = false
        }
    }

    // Parse citations and make them clickable
    function parseAnswer(text: string): { text: string; citations: string[] } {
        const citationRegex = /\[Source: ([^\]]+)\]/g
        const citations: string[] = []
        let match

        while ((match = citationRegex.exec(text)) !== null) {
            if (!citations.includes(match[1])) {
                citations.push(match[1])
            }
        }

        return { text, citations }
    }

    async function formatAnswerWithMarkdown(text: string): Promise<string> {
        // First convert [Source: Title] to simple placeholder
        const withPlaceholders = text.replace(/\[Source: ([^\]]+)\]/g, 'CITATIONSTART$1CITATIONEND')

        // Parse markdown
        const htmlContent = await marked.parse(withPlaceholders)

        // Convert placeholders back to clickable citation spans
        return htmlContent.replace(
            /CITATIONSTART(.+?)CITATIONEND/g,
            '<span class="citation" data-source="$1">[Source: $1]</span>',
        )
    }

    // Use runes instead of legacy reactive statements
    let parsedAnswer = $derived(parseAnswer(answer))
    let formattedAnswer = $state('')

    // Update formatted answer when answer changes
    $effect(() => {
        if (parsedAnswer.text) {
            formatAnswerWithMarkdown(parsedAnswer.text).then((html) => {
                formattedAnswer = html
            })
        } else {
            formattedAnswer = ''
        }
    })
</script>

<Card class="mb-6 border-blue-200 bg-blue-50">
    <CardHeader class="pb-3">
        <CardTitle class="flex items-center gap-2 text-lg">
            <Bot class="h-5 w-5 text-blue-600" />
            AI Answer
        </CardTitle>
    </CardHeader>
    <CardContent>
        {#if isLoading}
            <div class="flex items-center gap-2 text-gray-600">
                <Loader2 class="h-4 w-4 animate-spin" />
                Generating answer...
            </div>
        {:else if error}
            <div class="text-red-600">
                {error}
            </div>
        {:else if answer}
            <div class="prose prose-sm max-w-none">
                <!-- Use innerHTML to render citations as styled spans -->
                {@html formattedAnswer}
            </div>

            {#if parsedAnswer.citations.length > 0}
                <div class="mt-4 border-t border-blue-200 pt-3">
                    <h4 class="mb-2 text-sm font-medium text-gray-700">Sources:</h4>
                    <div class="flex flex-wrap gap-2">
                        {#each parsedAnswer.citations as citation}
                            <span
                                class="inline-flex items-center gap-1 rounded-md border border-blue-200 bg-white px-2 py-1 text-xs text-blue-700"
                            >
                                <ExternalLink class="h-3 w-3" />
                                {citation}
                            </span>
                        {/each}
                    </div>
                </div>
            {/if}
        {:else}
            <div class="text-gray-500">No answer generated.</div>
        {/if}
    </CardContent>
</Card>

<style>
    :global(.citation) {
        display: inline-flex;
        align-items: center;
        padding: 0.125rem 0.25rem;
        font-size: 0.75rem;
        line-height: 1rem;
        background-color: rgb(219 234 254);
        color: rgb(29 78 216);
        border-radius: 0.25rem;
        border: 1px solid rgb(191 219 254);
        cursor: pointer;
        transition-property: background-color;
        transition-timing-function: cubic-bezier(0.4, 0, 0.2, 1);
        transition-duration: 150ms;
    }

    :global(.citation:hover) {
        background-color: rgb(191 219 254);
    }
</style>
