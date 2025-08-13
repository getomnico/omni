<script lang="ts">
    import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js'
    import { Loader2, Sparkles, ExternalLink, ChevronDown, ChevronUp } from '@lucide/svelte'
    import type { SearchRequest } from '$lib/types/search.js'
    import { marked } from 'marked'

    interface Props {
        searchRequest: SearchRequest
    }

    let { searchRequest }: Props = $props()

    let isLoading = $state(true)
    let answer = $state('')
    let error = $state<string | null>(null)
    let isExpanded = $state(false)
    let shouldShowExpandButton = $state(false)
    let contentRef: HTMLDivElement | undefined = $state()

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

    // Parse markdown links and extract them as sources
    function parseAnswer(text: string): {
        text: string
        sources: Array<{ title: string; url: string }>
    } {
        const linkRegex = /\[([^\]]+)\]\(([^)]+)\)/g
        const sources: Array<{ title: string; url: string }> = []
        const seenUrls = new Set<string>()
        let match

        while ((match = linkRegex.exec(text)) !== null) {
            const [, title, url] = match
            if (!seenUrls.has(url)) {
                seenUrls.add(url)
                sources.push({ title, url })
            }
        }

        return { text, sources }
    }

    async function formatAnswerWithMarkdown(text: string): Promise<string> {
        // Parse markdown to HTML
        return await marked.parse(text)
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

    // Check if content exceeds max height after answer is rendered
    $effect(() => {
        if (formattedAnswer && contentRef) {
            // Use a small delay to ensure DOM is updated
            setTimeout(() => {
                if (contentRef) {
                    const scrollHeight = contentRef.scrollHeight
                    const maxHeight = 300 // 300px max height before showing "show more"
                    shouldShowExpandButton = scrollHeight > maxHeight
                }
            }, 10)
        }
    })
</script>

<Card class="mb-12 border-0 bg-gradient-to-br from-fuchsia-100 via-cyan-100 to-teal-100 shadow-lg">
    <CardHeader>
        <CardTitle class="flex items-center gap-2 text-lg">
            <Sparkles class="h-6 w-6 text-cyan-600" />
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
            <div>
                <div class="relative">
                    <div
                        bind:this={contentRef}
                        class="prose prose-sm max-w-none overflow-hidden transition-all duration-300"
                        class:max-h-[300px]={!isExpanded && shouldShowExpandButton}
                    >
                        <!-- Use innerHTML to render markdown with links -->
                        {@html formattedAnswer}
                    </div>

                    {#if !isExpanded && shouldShowExpandButton}
                        <div
                            class="pointer-events-none absolute -right-6 bottom-0 -left-6 h-20 bg-gradient-to-t from-teal-100 to-transparent"
                        ></div>
                    {/if}
                </div>

                {#if shouldShowExpandButton}
                    <button
                        onclick={() => (isExpanded = !isExpanded)}
                        class="mt-4 flex items-center gap-1 text-sm text-cyan-600 transition-colors hover:text-cyan-700"
                    >
                        {#if isExpanded}
                            <ChevronUp class="h-4 w-4" />
                            Show less
                        {:else}
                            <ChevronDown class="h-4 w-4" />
                            Show more
                        {/if}
                    </button>
                {/if}
            </div>

            {#if parsedAnswer.sources.length > 0}
                <div class="mt-4 border-t border-gray-200/50 pt-3">
                    <h4 class="mb-2 text-sm font-medium text-gray-700">Sources:</h4>
                    <div class="flex flex-wrap gap-2">
                        {#each parsedAnswer.sources as source}
                            <a
                                href={source.url}
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-flex items-center gap-1 rounded-md border border-gray-200 bg-white/80 px-2 py-1 text-xs text-gray-700 transition-all hover:bg-white hover:shadow-sm"
                            >
                                <ExternalLink class="h-3 w-3" />
                                {source.title}
                            </a>
                        {/each}
                    </div>
                </div>
            {/if}
        {:else}
            <div class="text-gray-500">No answer generated.</div>
        {/if}
    </CardContent>
</Card>
