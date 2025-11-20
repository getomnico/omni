<script lang="ts">
    import { marked, type Tokens, type RendererObject } from 'marked'
    import { mount } from 'svelte'
    import LinkHoverCard from './reflink-hover-card.svelte'
    import type { TextCitationParam } from '@anthropic-ai/sdk/resources.js'
    import type { CitationSearchResultLocationParam } from '@anthropic-ai/sdk/resources'

    type Props = {
        content: string
        citations?: TextCitationParam[]
    }

    let { content, citations }: Props = $props()
    $inspect(citations).with((t, v) => console.log('citations', t, v))
    let containerRef: HTMLElement | undefined = $state()

    const renderer: RendererObject = {
        link({ href, tokens }: Tokens.Link): string {
            const citation = citations?.find(
                (c) => c.type === 'search_result_location' && c.source === href,
            ) as CitationSearchResultLocationParam | null
            const text = this.parser.parseInline(tokens)
            return `<a href="${href}" class="omni-reflink" title="${citation?.title}" data-snippet="${citation?.cited_text}">${text}</a>`
        },
    }

    marked.use({ renderer })

    $effect(() => {
        if (!containerRef) {
            return
        }

        containerRef.innerHTML = marked.parse(content, { async: false })

        const linkPlaceholders = containerRef.querySelectorAll('.omni-reflink')
        linkPlaceholders.forEach((link) => {
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
                    text,
                    snippet: snippet || undefined,
                },
            })

            link.remove()
        })
    })
</script>

<div bind:this={containerRef}></div>
