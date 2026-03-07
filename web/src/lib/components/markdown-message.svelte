<script lang="ts">
    import { marked, type Tokens, type RendererObject } from 'marked'
    import { mount } from 'svelte'
    import LinkHoverCard from './reflink-hover-card.svelte'
    import {
        getSourceDisplayName,
        getSourceIconPath,
        getSourceTypeFromDisplayName,
    } from '$lib/utils/icons'
    import { SourceType } from '$lib/types'
    import type { TextCitationParam } from '@anthropic-ai/sdk/resources.js'
    import type { CitationSearchResultLocationParam } from '@anthropic-ai/sdk/resources'

    type Props = {
        content: string
        citations?: TextCitationParam[]
    }

    let { content, citations }: Props = $props()
    let containerRef: HTMLElement | undefined = $state()

    const appNames = Object.values(SourceType)
        .map((st) => getSourceDisplayName(st))
        .filter((name): name is string => !!name)
        .sort((a, b) => b.length - a.length)
    let injectedApps = new Set<string>()

    const injectAppIcon = (text: string): string => {
        const matches: { idx: number; appName: string; iconSrc: string }[] = []
        const textLower = text.toLowerCase()

        for (const appName of appNames) {
            if (injectedApps.has(appName)) continue
            const idx = textLower.indexOf(appName.toLowerCase())
            if (idx === -1) continue
            const sourceType = getSourceTypeFromDisplayName(appName)
            const iconSrc = sourceType ? getSourceIconPath(sourceType) : null
            if (!iconSrc) continue
            matches.push({ idx, appName, iconSrc })
        }

        matches.sort((a, b) => a.idx - b.idx)

        let result = ''
        let cursor = 0
        for (const m of matches) {
            if (m.idx < cursor) continue
            result += text.slice(cursor, m.idx)
            result += `<img src="${m.iconSrc}" alt="" style="display:inline;height:1em;width:1em;vertical-align:sub;margin-right:0.15em;margin-top:0;margin-bottom:0">`
            result += `<strong>${m.appName}</strong>`
            cursor = m.idx + m.appName.length
            injectedApps.add(m.appName)
        }
        result += text.slice(cursor)
        return result
    }

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

        text(tokens: Tokens.Text | Tokens.Escape | Tokens.Tag): string {
            let result: string
            if (tokens.type === 'text' && tokens.tokens) {
                result = this.parser.parseInline(tokens.tokens || [])
            } else {
                result = tokens.text
            }
            return injectAppIcon(result)
        },
    }

    marked.use({ renderer })

    $effect(() => {
        if (!containerRef) {
            return
        }

        injectedApps = new Set<string>()
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
