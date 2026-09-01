<script lang="ts">
    import { Marked, type Tokens, type RendererObject } from 'marked'
    import { mount, onDestroy, tick, unmount } from 'svelte'
    import { SvelteMap } from 'svelte/reactivity'
    import LinkHoverCard from './reflink-hover-card.svelte'
    import type { TextCitationParam } from '@anthropic-ai/sdk/resources'
    import { normalizeCitation, citationIdFromCitation } from '$lib/utils/citations'
    import { StreamingMarkdownRenderer } from '$lib/markdown/streaming-markdown-renderer'

    type Props = {
        content: string
        citations?: TextCitationParam[]
        isStreaming?: boolean
    }

    let { content, citations, isStreaming = false }: Props = $props()
    let staticContainerRef: HTMLElement | undefined = $state()
    let streamingContainerRef: HTMLElement | undefined = $state()
    let hasStreamed = $state(false)
    let staticCards: ReturnType<typeof mount>[] = []
    let streamingRenderer: StreamingMarkdownRenderer | undefined
    let streamingRendererRoot: HTMLElement | undefined
    let streamingCards = new SvelteMap<
        HTMLElement,
        { component: ReturnType<typeof mount>; signature: string }
    >()

    // Custom renderer only handles ordinary markdown links; citation placeholders
    // are pre-processed into inert spans before marked runs.
    const renderer: RendererObject = {
        link({ href, tokens }: Tokens.Link): string {
            const text = this.parser.parseInline(tokens)
            return `<a href="${href}" target="_blank" rel="noopener noreferrer">${text}</a>`
        },
    }
    const staticMarked = new Marked({ renderer })

    // Pre-process content: replace {omni-cit:SOURCE} placeholders with inert
    // <span> elements carrying a safe numeric index. The source identity is
    // URI-encoded so it survives text coalescing; we decode it here and find
    // the citation's position in the citations array.
    function preprocessContent(text: string, citationValues?: TextCitationParam[]): string {
        return text.replace(/\{omni-cit:([^}]+)\}/g, (_match, encodedId) => {
            let targetId: string
            try {
                targetId = decodeURIComponent(encodedId)
            } catch {
                return _match
            }
            const citationIdx =
                citationValues?.findIndex(
                    (citation) => citationIdFromCitation(citation) === targetId,
                ) ?? -1
            if (citationIdx >= 0) {
                return `<span class="omni-reflink" data-citation-idx="${citationIdx}"></span>`
            }
            return _match
        })
    }

    let useStreamingRenderer = $derived(isStreaming || hasStreamed)
    let renderedHtml = $derived(
        useStreamingRenderer
            ? ''
            : (staticMarked.parse(preprocessContent(content, citations), {
                  async: false,
              }) as string),
    )

    function mountStaticCitationCards(
        container: HTMLElement,
        citationValues?: TextCitationParam[],
    ) {
        for (const card of staticCards) unmount(card)
        staticCards = []

        const placeholders = Array.from(container.querySelectorAll('.omni-reflink'))
        for (const placeholder of placeholders) {
            const citationIdx = placeholder.getAttribute('data-citation-idx')
            const raw = citationIdx ? citationValues?.[Number.parseInt(citationIdx, 10)] : undefined
            const normalized = raw ? normalizeCitation(raw) : undefined
            staticCards.push(
                mount(LinkHoverCard, {
                    target: placeholder.parentNode as Element,
                    anchor: placeholder,
                    props: {
                        href: normalized?.href ?? null,
                        title: normalized?.title ?? '',
                        snippet: normalized?.citedText ?? undefined,
                        iconHint: normalized?.iconHint ?? null,
                        sourceName: normalized?.sourceName ?? 'Files',
                        locationLabel: normalized?.locationLabel ?? null,
                    },
                }),
            )
        }

        return placeholders
    }

    function removeStaticCitationPlaceholders(placeholders: Element[]) {
        for (const placeholder of placeholders) {
            let previousSibling = placeholder.previousSibling
            while (previousSibling instanceof Text && previousSibling.textContent?.trim() === '') {
                const whitespaceNode = previousSibling
                previousSibling = previousSibling.previousSibling
                whitespaceNode.remove()
            }
            placeholder.remove()
        }
    }

    function syncStreamingCitationCards(
        container: HTMLElement,
        citationValues?: TextCitationParam[],
    ): void {
        for (const [placeholder, entry] of streamingCards) {
            if (!placeholder.isConnected || !container.contains(placeholder)) {
                unmount(entry.component)
                streamingCards.delete(placeholder)
            }
        }

        for (const placeholder of Array.from(
            container.querySelectorAll<HTMLElement>('.omni-reflink'),
        )) {
            const rawIndex = placeholder.getAttribute('data-citation-idx')
            if (rawIndex === null) continue
            const citation = citationValues?.[Number.parseInt(rawIndex, 10)]
            if (!citation) continue

            const signature = citationIdFromCitation(citation)
            const existing = streamingCards.get(placeholder)
            if (existing?.signature === signature) continue
            if (existing) {
                unmount(existing.component)
                streamingCards.delete(placeholder)
            }

            const normalized = normalizeCitation(citation)
            const component = mount(LinkHoverCard, {
                target: placeholder.parentNode as Element,
                anchor: placeholder,
                props: {
                    href: normalized.href,
                    title: normalized.title,
                    snippet: normalized.citedText,
                    iconHint: normalized.iconHint,
                    sourceName: normalized.sourceName,
                    locationLabel: normalized.locationLabel,
                },
            })
            streamingCards.set(placeholder, { component, signature })
        }
    }

    // Once streaming starts, keep this component on the incremental DOM path even
    // after end_of_stream so the last chunk's DOM and animation are not discarded.
    $effect(() => {
        if (isStreaming) hasStreamed = true
    })

    // Existing, non-streaming messages retain the SSR-friendly HTML path.
    $effect(() => {
        void renderedHtml
        const container = staticContainerRef
        if (!container || useStreamingRenderer) return

        let cancelled = false
        tick().then(() => {
            if (cancelled || staticContainerRef !== container || useStreamingRenderer) return
            const placeholders = mountStaticCitationCards(container, citations)
            void tick().then(() => {
                if (!cancelled && staticContainerRef === container && !useStreamingRenderer) {
                    removeStaticCitationPlaceholders(placeholders)
                }
            })
        })

        return () => {
            cancelled = true
            for (const card of staticCards) unmount(card)
            staticCards = []
        }
    })

    // The renderer receives cumulative content snapshots, but commits at most once
    // per animation frame and preserves all DOM nodes it can prove compatible.
    $effect(() => {
        const container = streamingContainerRef
        if (!container || !useStreamingRenderer) return

        let renderer = streamingRenderer
        if (streamingRendererRoot !== container || !renderer) {
            renderer?.destroy()
            renderer = new StreamingMarkdownRenderer(container)
            streamingRenderer = renderer
            streamingRendererRoot = container
        }

        const citationValues = citations ? [...citations] : undefined
        renderer.enqueue({
            source: preprocessContent(content, citationValues),
            isStreaming: isStreaming || hasStreamed,
            onCommit: () => syncStreamingCitationCards(container, citationValues),
        })
    })

    onDestroy(() => {
        streamingRenderer?.destroy()
        for (const card of staticCards) unmount(card)
        for (const { component } of streamingCards.values()) unmount(component)
        staticCards = []
        streamingCards.clear()
    })
</script>

{#if useStreamingRenderer}
    <div bind:this={streamingContainerRef}></div>
{:else}
    <!-- Marked output preserves the existing renderer's raw HTML behavior. -->
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div bind:this={staticContainerRef}>{@html renderedHtml}</div>
{/if}

<style>
    :global(.omni-streaming-chunk) {
        animation: omni-streaming-fade-in 220ms ease-out both;
    }

    @keyframes omni-streaming-fade-in {
        from {
            opacity: 0;
        }
        to {
            opacity: 1;
        }
    }

    @media (prefers-reduced-motion: reduce) {
        :global(.omni-streaming-chunk) {
            animation: none;
        }
    }
</style>
