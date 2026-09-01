import { Marked, Parser, TextRenderer, type Token, type Tokens } from 'marked'
import {
    createMarkdownParser,
    encodeMarkdownUrl,
    MARKDOWN_LINK_REL,
    MARKDOWN_LINK_TARGET,
} from './marked'

export type StreamingMarkdownSnapshot = {
    source: string
    isStreaming: boolean
    onCommit?: () => void
}

type Slot = {
    element: HTMLElement
    created: boolean
    replaced: boolean
}

const KIND_ATTRIBUTE = 'data-omni-markdown-kind'
const TEXT_KIND = 'text'
const VOID_HTML_ELEMENTS = new Set([
    'area',
    'base',
    'br',
    'col',
    'embed',
    'hr',
    'img',
    'input',
    'link',
    'meta',
    'param',
    'source',
    'track',
    'wbr',
])

/**
 * Renders successive complete Markdown snapshots into one owned DOM tree.
 * Marked still parses the complete source for correctness; this class only
 * makes the DOM commit append-only wherever the token shape is unchanged.
 */
export class StreamingMarkdownRenderer {
    private readonly marked: Marked
    private readonly inlineMarked = createMarkdownParser()
    private readonly textParser = new Parser()
    private readonly textRenderer = new TextRenderer()
    private readonly textValues = new WeakMap<HTMLElement, string>()
    private pendingSnapshot: StreamingMarkdownSnapshot | null = null
    private animationFrame: number | null = null
    private lastReceivedSource: string | null = null
    private lastCommittedSource: string | null = null
    private animateCurrentCommit = false
    private destroyed = false

    constructor(public readonly root: HTMLElement) {
        this.marked = new Marked({
            hooks: {
                provideParser: () => (tokens) => {
                    this.renderTokens(tokens)
                    return ''
                },
            },
        })
    }

    enqueue(snapshot: StreamingMarkdownSnapshot): void {
        if (this.destroyed) return

        const isAppend =
            this.lastReceivedSource !== null &&
            snapshot.source.startsWith(this.lastReceivedSource) &&
            snapshot.source.length > this.lastReceivedSource.length
        this.lastReceivedSource = snapshot.source

        if (this.lastCommittedSource === null) {
            this.commit({ ...snapshot, isStreaming: false })
            return
        }

        const preservesPendingAnimation =
            this.pendingSnapshot?.source === snapshot.source && this.pendingSnapshot.isStreaming
        this.pendingSnapshot = {
            ...snapshot,
            isStreaming: (snapshot.isStreaming && isAppend) || preservesPendingAnimation,
        }
        this.scheduleCommit()
    }

    destroy(): void {
        this.destroyed = true
        if (this.animationFrame !== null) {
            cancelAnimationFrame(this.animationFrame)
            this.animationFrame = null
        }
        this.pendingSnapshot = null
        this.root.replaceChildren()
    }

    private scheduleCommit(): void {
        if (this.animationFrame !== null) return
        this.animationFrame = requestAnimationFrame(() => this.flushPendingSnapshot())
    }

    private flushPendingSnapshot(): void {
        this.animationFrame = null
        const snapshot = this.pendingSnapshot
        this.pendingSnapshot = null
        if (!snapshot || this.destroyed) return

        this.commit(snapshot)
        if (this.pendingSnapshot) this.scheduleCommit()
    }

    private commit(snapshot: StreamingMarkdownSnapshot): void {
        if (this.destroyed) return
        if (snapshot.source === this.lastCommittedSource) {
            snapshot.onCommit?.()
            return
        }

        const shouldAnimate =
            snapshot.isStreaming &&
            this.lastCommittedSource !== null &&
            snapshot.source.startsWith(this.lastCommittedSource) &&
            snapshot.source.length > this.lastCommittedSource.length

        this.animateCurrentCommit = shouldAnimate
        this.marked.parse(snapshot.source, { async: false })
        this.lastCommittedSource = snapshot.source
        snapshot.onCommit?.()
    }

    private renderTokens(tokens: Token[]): void {
        this.patchBlocks(this.root, tokens, this.animateCurrentCommit, true)
    }

    private createElement<T extends keyof HTMLElementTagNameMap>(
        tagName: T,
        kind: string,
    ): HTMLElementTagNameMap[T] {
        const element = document.createElement(tagName)
        element.setAttribute(KIND_ATTRIBUTE, kind)
        return element
    }

    private ensureElement(parent: Element, index: number, tagName: string, kind: string): Slot {
        const current = parent.children[index]
        if (
            current instanceof HTMLElement &&
            current.tagName.toLowerCase() === tagName &&
            current.getAttribute(KIND_ATTRIBUTE) === kind
        ) {
            return { element: current, created: false, replaced: false }
        }

        const element = this.createElement(tagName as keyof HTMLElementTagNameMap, kind)
        if (current) {
            current.replaceWith(element)
            return { element, created: false, replaced: true }
        }
        parent.append(element)
        return { element, created: true, replaced: false }
    }

    private trimChildren(parent: Element, length: number): void {
        while (parent.children.length > length) parent.lastElementChild?.remove()
    }

    private appendText(target: HTMLElement, text: string, animate: boolean): void {
        if (!text) return
        const chunk = document.createElement('span')
        if (animate) chunk.classList.add('omni-streaming-chunk')
        chunk.textContent = text
        target.append(chunk)
    }

    private patchText(parent: Element, index: number, text: string, animate: boolean): number {
        const slot = this.ensureElement(parent, index, 'span', TEXT_KIND)
        const target = slot.element
        const previousText = this.textValues.get(target) ?? target.textContent ?? ''

        if (previousText !== text) {
            if (text.startsWith(previousText)) {
                this.appendText(target, text.slice(previousText.length), animate)
            } else {
                // Markdown may reinterpret the tail when a delimiter closes.
                target.replaceChildren(document.createTextNode(text))
            }
            this.textValues.set(target, text)
        }
        return index + 1
    }

    private patchBlocks(
        parent: Element,
        tokens: Token[],
        animate: boolean,
        top: boolean,
        startIndex = 0,
    ): number {
        let outputIndex = startIndex

        for (const token of tokens) {
            if (token.type === 'space' || token.type === 'def') continue

            if (token.type === 'paragraph') {
                const slot = this.ensureElement(parent, outputIndex, 'p', 'paragraph')
                this.patchInlineContents(
                    slot.element,
                    token.tokens as Token[],
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'heading') {
                const slot = this.ensureElement(parent, outputIndex, `h${token.depth}`, 'heading')
                this.patchInlineContents(
                    slot.element,
                    token.tokens as Token[],
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'list') {
                const slot = this.ensureElement(
                    parent,
                    outputIndex,
                    token.ordered ? 'ol' : 'ul',
                    'list',
                )
                this.patchList(
                    slot.element,
                    token as Tokens.List,
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'blockquote') {
                const slot = this.ensureElement(parent, outputIndex, 'blockquote', 'blockquote')
                this.patchBlocks(
                    slot.element,
                    token.tokens as Token[],
                    slot.created ? animate : animate && !slot.replaced,
                    true,
                )
                outputIndex++
                continue
            }

            if (token.type === 'table') {
                const slot = this.ensureElement(parent, outputIndex, 'table', 'table')
                this.patchTable(
                    slot.element,
                    token as Tokens.Table,
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'code') {
                const pre = this.ensureElement(parent, outputIndex, 'pre', 'code-block')
                const code = this.ensureElement(pre.element, 0, 'code', 'code').element
                const language = token.lang?.match(/^\S*/)?.[0]
                if (language) code.className = `language-${language}`
                else code.removeAttribute('class')
                const text = `${token.text.replace(/\n$/, '')}\n`
                this.patchText(code, 0, text, pre.created ? animate : animate && !pre.replaced)
                this.trimChildren(pre.element, 1)
                outputIndex++
                continue
            }

            if (token.type === 'hr') {
                this.ensureElement(parent, outputIndex, 'hr', 'hr')
                outputIndex++
                continue
            }

            if (token.type === 'html') {
                this.patchRawHtml(parent, outputIndex, token.raw)
                outputIndex++
                continue
            }

            if (token.type === 'text') {
                if (top) {
                    const slot = this.ensureElement(parent, outputIndex, 'p', 'implicit-paragraph')
                    this.patchInlineContents(
                        slot.element,
                        (token.tokens ?? [token]) as Token[],
                        slot.created ? animate : animate && !slot.replaced,
                    )
                    outputIndex++
                } else {
                    outputIndex = this.patchInlineRange(
                        parent,
                        (token.tokens ?? [token]) as Token[],
                        animate,
                        outputIndex,
                    )
                }
                continue
            }

            throw new Error(`Unsupported block token: ${token.type}`)
        }

        this.trimChildren(parent, outputIndex)
        return outputIndex
    }

    private patchInlineContents(
        parent: Element,
        tokens: Token[],
        animate: boolean,
        startIndex = 0,
    ): number {
        const outputIndex = this.patchInlineRange(parent, tokens, animate, startIndex)
        this.trimChildren(parent, outputIndex)
        return outputIndex
    }

    private patchInlineRange(
        parent: Element,
        tokens: Token[],
        animate: boolean,
        startIndex = 0,
    ): number {
        let outputIndex = startIndex

        for (let tokenIndex = 0; tokenIndex < tokens.length; tokenIndex++) {
            const token = tokens[tokenIndex]

            if (token.type === 'text' || token.type === 'escape') {
                if (token.type === 'text' && token.tokens) {
                    outputIndex = this.patchInlineRange(parent, token.tokens, animate, outputIndex)
                } else {
                    outputIndex = this.patchText(parent, outputIndex, token.text, animate)
                }
                continue
            }

            if (token.type === 'strong' || token.type === 'em' || token.type === 'del') {
                const slot = this.ensureElement(parent, outputIndex, token.type, token.type)
                this.patchInlineContents(
                    slot.element,
                    token.tokens as Token[],
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'codespan') {
                const slot = this.ensureElement(parent, outputIndex, 'code', 'codespan')
                this.patchText(
                    slot.element,
                    0,
                    token.text,
                    slot.created ? animate : animate && !slot.replaced,
                )
                this.trimChildren(slot.element, 1)
                outputIndex++
                continue
            }

            if (token.type === 'link') {
                const href = encodeMarkdownUrl(token.href)
                if (href === null) {
                    outputIndex = this.patchInlineRange(
                        parent,
                        token.tokens as Token[],
                        animate,
                        outputIndex,
                    )
                    continue
                }

                const slot = this.ensureElement(parent, outputIndex, 'a', 'link')
                slot.element.setAttribute('href', href)
                slot.element.setAttribute('target', MARKDOWN_LINK_TARGET)
                slot.element.setAttribute('rel', MARKDOWN_LINK_REL)
                if (token.title) slot.element.setAttribute('title', token.title)
                else slot.element.removeAttribute('title')
                this.patchInlineContents(
                    slot.element,
                    token.tokens as Token[],
                    slot.created ? animate : animate && !slot.replaced,
                )
                outputIndex++
                continue
            }

            if (token.type === 'image') {
                const alt = this.textParser.parseInline(token.tokens as Token[], this.textRenderer)
                const href = encodeMarkdownUrl(token.href)
                if (href === null) {
                    outputIndex = this.patchText(parent, outputIndex, alt, animate)
                    continue
                }

                const slot = this.ensureElement(parent, outputIndex, 'img', 'image')
                slot.element.setAttribute('src', href)
                slot.element.setAttribute('alt', alt)
                if (token.title) slot.element.setAttribute('title', token.title)
                else slot.element.removeAttribute('title')
                outputIndex++
                continue
            }

            if (token.type === 'br') {
                this.ensureElement(parent, outputIndex, 'br', 'br')
                outputIndex++
                continue
            }

            if (token.type === 'html') {
                const groupEnd = this.inlineHtmlGroupEnd(tokens, tokenIndex)
                const raw = tokens
                    .slice(tokenIndex, groupEnd)
                    .map((groupToken) => groupToken.raw)
                    .join('')
                const html = this.inlineMarked.parseInline(raw, { async: false })
                this.patchRawHtml(parent, outputIndex, raw, html)
                outputIndex++
                tokenIndex = groupEnd - 1
                continue
            }

            throw new Error(`Unsupported inline token: ${token.type}`)
        }

        return outputIndex
    }

    private inlineHtmlGroupEnd(tokens: Token[], startIndex: number): number {
        const token = tokens[startIndex]
        if (token.type !== 'html') return startIndex + 1

        const openingTag = /^<([a-z][a-z0-9:-]*)(?=[\s/>])/i.exec(token.raw.trim())
        if (!openingTag || token.raw.trimEnd().endsWith('/>')) return startIndex + 1

        const tagName = openingTag[1].toLowerCase()
        if (VOID_HTML_ELEMENTS.has(tagName)) return startIndex + 1

        let depth = 1
        for (let index = startIndex + 1; index < tokens.length; index++) {
            const candidate = tokens[index]
            if (candidate.type !== 'html') continue

            const raw = candidate.raw.trim()
            const closingTag = /^<\/([a-z][a-z0-9:-]*)\s*>/i.exec(raw)
            if (closingTag?.[1].toLowerCase() === tagName) {
                depth--
                if (depth === 0) return index + 1
                continue
            }

            const nestedOpeningTag = /^<([a-z][a-z0-9:-]*)(?=[\s/>])/i.exec(raw)
            if (
                nestedOpeningTag?.[1].toLowerCase() === tagName &&
                !raw.endsWith('/>') &&
                !VOID_HTML_ELEMENTS.has(tagName)
            ) {
                depth++
            }
        }

        return tokens.length
    }

    private patchList(parent: HTMLElement, token: Tokens.List, animate: boolean): void {
        if (token.ordered && token.start !== 1 && token.start !== '') {
            parent.setAttribute('start', String(token.start))
        } else {
            parent.removeAttribute('start')
        }

        for (let index = 0; index < token.items.length; index++) {
            const itemToken = token.items[index]
            const slot = this.ensureElement(parent, index, 'li', 'list-item')
            this.patchListItem(
                slot.element,
                itemToken,
                slot.created ? animate : animate && !slot.replaced,
            )
        }
        this.trimChildren(parent, token.items.length)
    }

    private patchListItem(parent: HTMLElement, token: Tokens.ListItem, animate: boolean): void {
        let startIndex = 0
        const tight = !token.loose

        if (token.task && tight) {
            this.patchCheckbox(parent, 0, token.checked === true)
            this.patchStaticSpace(parent, 1)
            startIndex = 2
        }

        const firstToken = token.tokens[0]
        if (token.task && !tight && firstToken?.type === 'paragraph') {
            const first = firstToken
            const paragraphSlot = this.ensureElement(parent, 0, 'p', 'paragraph')
            this.patchCheckbox(paragraphSlot.element, 0, token.checked === true)
            this.patchStaticSpace(paragraphSlot.element, 1)
            this.patchInlineContents(paragraphSlot.element, first.tokens as Token[], animate, 2)
            startIndex = 1
            this.patchBlocks(parent, token.tokens.slice(1), animate, true, startIndex)
        } else {
            this.patchBlocks(parent, token.tokens, animate, !tight, startIndex)
        }
    }

    private patchCheckbox(parent: Element, index: number, checked: boolean): void {
        const slot = this.ensureElement(parent, index, 'input', 'task-checkbox')
        slot.element.setAttribute('type', 'checkbox')
        slot.element.setAttribute('disabled', '')
        if (checked) slot.element.setAttribute('checked', '')
        else slot.element.removeAttribute('checked')
        slot.element.toggleAttribute('checked', checked)
        ;(slot.element as HTMLInputElement).checked = checked
        ;(slot.element as HTMLInputElement).disabled = true
    }

    private patchStaticSpace(parent: Element, index: number): void {
        const slot = this.ensureElement(parent, index, 'span', 'task-space')
        slot.element.textContent = ' '
    }

    private patchTable(parent: HTMLElement, token: Tokens.Table, animate: boolean): void {
        const head = this.ensureElement(parent, 0, 'thead', 'table-head').element
        const headerRow = this.ensureElement(head, 0, 'tr', 'table-header-row').element
        this.patchTableCells(headerRow, token.header, animate, true)

        if (token.rows.length === 0) {
            this.trimChildren(parent, 1)
            return
        }

        const body = this.ensureElement(parent, 1, 'tbody', 'table-body').element
        for (let rowIndex = 0; rowIndex < token.rows.length; rowIndex++) {
            const row = this.ensureElement(body, rowIndex, 'tr', 'table-row').element
            this.patchTableCells(row, token.rows[rowIndex], animate, false)
        }
        this.trimChildren(body, token.rows.length)
        this.trimChildren(parent, 2)
    }

    private patchTableCells(
        row: HTMLElement,
        cells: Tokens.TableCell[],
        animate: boolean,
        header: boolean,
    ): void {
        for (let index = 0; index < cells.length; index++) {
            const cellToken = cells[index]
            const tagName = header ? 'th' : 'td'
            const kind = header ? 'table-header-cell' : 'table-cell'
            const slot = this.ensureElement(row, index, tagName, kind)
            if (cellToken.align) slot.element.setAttribute('align', cellToken.align)
            else slot.element.removeAttribute('align')
            this.patchInlineContents(
                slot.element,
                cellToken.tokens,
                slot.created ? animate : animate && !slot.replaced,
            )
        }
        this.trimChildren(row, cells.length)
    }

    private patchRawHtml(parent: Element, index: number, raw: string, html = raw): void {
        const slot = this.ensureElement(parent, index, 'span', 'raw-html')
        if (slot.element.dataset.omniRawHtml === raw) return

        const template = document.createElement('template')
        template.innerHTML = html
        slot.element.replaceChildren(...Array.from(template.content.childNodes))
        slot.element.dataset.omniRawHtml = raw
        slot.element.style.display = 'contents'
    }
}
