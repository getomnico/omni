<script lang="ts">
    import { marked, type Tokens, type RendererObject } from 'marked'
    import { mount, unmount, tick } from 'svelte'
    import LinkHoverCard from './reflink-hover-card.svelte'
    import type { TextCitationParam } from '@anthropic-ai/sdk/resources'
    import { normalizeCitation, citationIdFromCitation } from '$lib/utils/citations'

    type Props = {
        content: string
        citations?: TextCitationParam[]
    }

    let { content, citations }: Props = $props()
    let containerRef: HTMLElement | undefined = $state()
    let mountedCards: ReturnType<typeof mount>[] = []

    // Custom renderer only handles ordinary markdown links; citation placeholders
    // are pre-processed into inert spans before marked runs.
    const renderer: RendererObject = {
        link({ href, tokens }: Tokens.Link): string {
            const text = this.parser.parseInline(tokens)
            return `<a href="${href}" target="_blank" rel="noopener noreferrer">${text}</a>`
        },
    }

    marked.use({ renderer })

    // Pre-process content: replace {omni-cit:SOURCE} placeholders with inert
    // <span> elements carrying a safe numeric index. The source identity is
    // URI-encoded so it survives text coalescing; we decode it here and find
    // the citation's position in the citations array.
    function preprocessContent(text: string): string {
        return text.replace(/\{omni-cit:([^}]+)\}/g, (_match, encodedId) => {
            let targetId: string
            try {
                targetId = decodeURIComponent(encodedId)
            } catch {
                return _match
            }
            const citationIdx =
                citations?.findIndex((c) => citationIdFromCitation(c) === targetId) ?? -1
            if (citationIdx >= 0) {
                return `<span class="omni-reflink" data-citation-idx="${citationIdx}"></span>`
            }
            return _match
        })
    }

    let renderedHtml = $derived(
        marked.parse(preprocessContent(content), { async: false }) as string,
    )

    // Reactive effect: re-mount hover cards when rendered HTML changes.
    // Cleanup on re-run or destroy unmounts all mounted cards.
    $effect(() => {
        const html = renderedHtml
        const container = containerRef

        if (!container) return
        let cancelled = false

        tick().then(async () => {
            if (cancelled || renderedHtml !== html || containerRef !== container) return

            // Unmount existing cards
            for (const card of mountedCards) {
                unmount(card)
            }
            mountedCards = []

            const linkPlaceholders = Array.from(container.querySelectorAll('.omni-reflink'))
            for (const link of linkPlaceholders) {
                const citationIdx = link.getAttribute('data-citation-idx')
                const raw = citationIdx ? citations?.[parseInt(citationIdx, 10)] : undefined
                const normalized = raw ? normalizeCitation(raw) : undefined
                mountedCards.push(
                    mount(LinkHoverCard, {
                        target: link.parentNode as Element,
                        anchor: link,
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

            await tick()
            if (cancelled || renderedHtml !== html || containerRef !== container) return

            for (const link of linkPlaceholders) {
                let previousSibling = link.previousSibling
                while (
                    previousSibling instanceof Text &&
                    previousSibling.textContent?.trim() === ''
                ) {
                    const whitespaceNode = previousSibling
                    previousSibling = previousSibling.previousSibling
                    whitespaceNode.remove()
                }
                link.remove()
            }
        })

        return () => {
            cancelled = true
            for (const card of mountedCards) {
                unmount(card)
            }
            mountedCards = []
        }
    })
</script>

<div bind:this={containerRef}>{@html renderedHtml}</div>
