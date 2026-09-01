import { Marked, type RendererObject } from 'marked'

export const MARKDOWN_LINK_TARGET = '_blank'
export const MARKDOWN_LINK_REL = 'noopener noreferrer'

const HTML_ATTRIBUTE_CHARACTERS = /[&<>"']/g
const HTML_ATTRIBUTE_ENTITIES: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
}

function escapeHtmlAttribute(value: string): string {
    return value.replace(HTML_ATTRIBUTE_CHARACTERS, (character) => {
        const entity = HTML_ATTRIBUTE_ENTITIES[character]
        if (!entity) throw new Error(`Unsupported HTML attribute character: ${character}`)
        return entity
    })
}

export function encodeMarkdownUrl(value: string): string | null {
    try {
        return encodeURI(value).replace(/%25/g, '%')
    } catch {
        return null
    }
}

function createRenderer(): RendererObject {
    return {
        link({ href, title, tokens }): string {
            const text = this.parser.parseInline(tokens)
            const encodedHref = encodeMarkdownUrl(href)
            if (encodedHref === null) return text

            const titleAttribute = title ? ` title="${escapeHtmlAttribute(title)}"` : ''
            return `<a href="${escapeHtmlAttribute(encodedHref)}" target="${MARKDOWN_LINK_TARGET}" rel="${MARKDOWN_LINK_REL}"${titleAttribute}>${text}</a>`
        },
        image({ href, title, text, tokens }): string {
            const alt = tokens ? this.parser.parseInline(tokens, this.parser.textRenderer) : text
            const encodedHref = encodeMarkdownUrl(href)
            if (encodedHref === null) return escapeHtmlAttribute(alt)

            const titleAttribute = title ? ` title="${escapeHtmlAttribute(title)}"` : ''
            return `<img src="${escapeHtmlAttribute(encodedHref)}" alt="${escapeHtmlAttribute(alt)}"${titleAttribute}>`
        },
    }
}

export function createMarkdownParser(): Marked {
    return new Marked({ renderer: createRenderer() })
}
