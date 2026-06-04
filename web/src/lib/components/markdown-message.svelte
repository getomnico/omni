<script lang="ts">
    import { marked, type Tokens, type RendererObject } from 'marked'
    import { mount, tick } from 'svelte'
    import LinkHoverCard from './reflink-hover-card.svelte'
    import type { TextCitationParam } from '@anthropic-ai/sdk/resources.js'
    import type { CitationSearchResultLocationParam } from '@anthropic-ai/sdk/resources'

    type Props = {
        content: string
        citations?: TextCitationParam[]
    }

    let { content, citations }: Props = $props()
    let containerRef: HTMLElement | undefined = $state()

    const renderer: RendererObject = {
        link({ href, tokens }: Tokens.Link): string {
            const citation = citations?.find(
                (c) => c.type === 'search_result_location' && c.source === href,
            ) as CitationSearchResultLocationParam | null

            const text = this.parser.parseInline(tokens)
            if (citation) {
                return `<a href="${href}" class="omni-reflink" title="${citation?.title}" data-snippet="${citation?.cited_text}">${text}</a>`
            } else {
                return `<a href="${href}">${text}</a>`
            }
        },
    }

    marked.use({ renderer })

    let renderedHtml = $derived(marked.parse(content, { async: false }) as string)

    async function mountReflinkHoverCards(html: string, container: HTMLElement | undefined) {
        if (!container) return

        await tick()
        if (renderedHtml !== html || containerRef !== container) return

        const linkPlaceholders = Array.from(container.querySelectorAll('.omni-reflink'))
        for (const link of linkPlaceholders) {
            const href = link.getAttribute('href')
            const title = link.getAttribute('title')
            const text = link.textContent
            const snippet = link.getAttribute('data-snippet')

            mount(LinkHoverCard, {
                target: link.parentNode as Element,
                anchor: link,
                props: {
                    href: href || '#',
                    title: title || '',
                    text: text ?? '',
                    snippet: snippet || undefined,
                },
            })
        }

        await tick()
        if (renderedHtml !== html || containerRef !== container) return

        for (const link of linkPlaceholders) {
            let previousSibling = link.previousSibling
            while (previousSibling instanceof Text && previousSibling.textContent?.trim() === '') {
                const whitespaceNode = previousSibling
                previousSibling = previousSibling.previousSibling
                whitespaceNode.remove()
            }

            link.remove()
        }
    }

    $effect(() => {
        void mountReflinkHoverCards(renderedHtml, containerRef)
    })
</script>

<div bind:this={containerRef}>{@html renderedHtml}</div>
