import { build } from 'vite'
import { expect, test, type Page } from '@playwright/test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

type Renderer = {
    enqueue(snapshot: { source: string; isStreaming: boolean }): void
}

declare global {
    interface Window {
        StreamingMarkdownTest: {
            StreamingMarkdownRenderer: new (root: HTMLElement) => Renderer
            renderMarkdown(source: string): string
        }
        streamingRenderer: Renderer
        rendererRefs: {
            nestedList: Element
            nestedListItem: Element
            nestedText: Element
            nestedChunk: Element
        }
    }
}

const rendererModule = resolve('src/lib/markdown/streaming-markdown-renderer.ts')
const markedModule = resolve('src/lib/markdown/marked.ts')

async function buildBrowserBundle(): Promise<{ directory: string; bundle: string }> {
    const directory = await mkdtemp(join(tmpdir(), 'omni-streaming-markdown-'))
    const sourceDirectory = join(directory, 'source')
    const outputDirectory = join(directory, 'output')
    const entry = join(sourceDirectory, 'entry.ts')
    const bundle = join(outputDirectory, 'renderer.js')
    await mkdir(sourceDirectory)
    await writeFile(
        entry,
        `import { createMarkdownParser } from ${JSON.stringify(markedModule)}
import { StreamingMarkdownRenderer } from ${JSON.stringify(rendererModule)}
const parser = createMarkdownParser()
globalThis.StreamingMarkdownTest = {
    StreamingMarkdownRenderer,
    renderMarkdown: (source) => parser.parse(source, { async: false }),
}
`,
    )
    try {
        await build({
            configFile: false,
            root: sourceDirectory,
            build: {
                emptyOutDir: true,
                outDir: outputDirectory,
                rollupOptions: {
                    input: entry,
                    output: {
                        entryFileNames: 'renderer.js',
                        format: 'iife',
                        name: 'OmniMarkdownTestBundle',
                    },
                },
            },
        })
        return { directory, bundle }
    } catch (error) {
        await rm(directory, { recursive: true, force: true })
        throw error
    }
}

async function update(page: Page, source: string) {
    await page.evaluate((markdown: string) => {
        window.streamingRenderer.enqueue({ source: markdown, isStreaming: true })
    }, source)
    await page.evaluate(
        () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
    )
}

test('preserves streamed Markdown nodes and matches static Markdown semantics', async ({
    page,
}) => {
    const { directory, bundle } = await buildBrowserBundle()
    try {
        await page.setContent('<main id="root"></main>')
        await page.addScriptTag({ path: bundle })
        await page.evaluate(() => {
            const root = document.querySelector('#root')
            if (!root) throw new Error('Renderer root was not found')
            window.streamingRenderer = new window.StreamingMarkdownTest.StreamingMarkdownRenderer(
                root,
            )
        })

        await update(
            page,
            '*   **High-Quality Feedback Loops**\n    *   The Hacker News audience is',
        )
        await page.evaluate(() => {
            const nestedList = document.querySelector('#root > ul > li > ul')
            const nestedListItem = nestedList?.querySelector('li')
            const nestedText = nestedListItem?.querySelector('[data-omni-markdown-kind="text"]')
            const nestedChunk = nestedText?.querySelector('span')
            if (!nestedList || !nestedListItem || !nestedText || !nestedChunk) {
                throw new Error('Nested list test structure was not rendered')
            }
            window.rendererRefs = { nestedList, nestedListItem, nestedText, nestedChunk }
        })

        await update(
            page,
            '*   **High-Quality Feedback Loops**\n    *   The Hacker News audience is notoriously technical',
        )
        const nestedResult = await page.evaluate(() => {
            const nestedList = document.querySelector('#root > ul > li > ul')
            const nestedListItem = nestedList?.querySelector('li')
            const nestedText = nestedListItem?.querySelector('[data-omni-markdown-kind="text"]')
            const chunks = nestedText?.children
            return {
                listPreserved: nestedList === window.rendererRefs.nestedList,
                itemPreserved: nestedListItem === window.rendererRefs.nestedListItem,
                textPreserved: nestedText === window.rendererRefs.nestedText,
                firstChunkPreserved: chunks?.[0] === window.rendererRefs.nestedChunk,
                chunkTexts: chunks ? Array.from(chunks, (chunk) => chunk.textContent) : [],
                suffixFades: chunks?.[1]?.classList.contains('omni-streaming-chunk') ?? false,
            }
        })
        expect(nestedResult).toEqual({
            listPreserved: true,
            itemPreserved: true,
            textPreserved: true,
            firstChunkPreserved: true,
            chunkTexts: ['The Hacker News audience is', ' notoriously technical'],
            suffixFades: true,
        })

        const corpus = [
            '# Heading',
            '> quoted text',
            '- [x] done\n- [ ] todo',
            '```js\nconst value = 1\n```',
            '[link](https://example.com "Example")',
            String.raw`[safe](https://example.com "a\" onmouseover=\"alert(1)")`,
            '[spaced](<https://example.com/a b> "Space")',
            '![**bold** & `code`](<https://example.com/a b.png> "Image")',
            String.raw`![safe\" onerror=\"alert(1)](image.png)`,
            '| A | B |\n| :--- | ---: |\n| 1 | 2 |',
            '<div>raw html</div>',
            'This is <mark>**highlighted**</mark>.',
        ]
        for (const source of corpus) {
            const parity = await page.evaluate((markdown) => {
                const normalize = (root: Element): unknown => {
                    type NodeValue =
                        | { type: 'text'; value: string }
                        | {
                              type: 'element'
                              tag: string
                              attributes: string[]
                              children: unknown[]
                          }

                    const appendValue = (
                        target: NodeValue[],
                        value: NodeValue | NodeValue[] | null,
                    ) => {
                        const values = value === null ? [] : Array.isArray(value) ? value : [value]
                        for (const item of values) {
                            const previous = target.at(-1)
                            if (previous?.type === 'text' && item.type === 'text') {
                                previous.value += item.value
                            } else {
                                target.push(item)
                            }
                        }
                    }

                    const visit = (
                        node: Node,
                        preserveWhitespace = false,
                    ): NodeValue | NodeValue[] | null => {
                        if (node.nodeType === Node.TEXT_NODE) {
                            const value = node.textContent ?? ''
                            return !preserveWhitespace && value.trim() === ''
                                ? null
                                : { type: 'text', value }
                        }
                        if (node.nodeType !== Node.ELEMENT_NODE) return null

                        const element = node as HTMLElement
                        const managedKind = element.getAttribute('data-omni-markdown-kind')
                        if (
                            managedKind === 'text' ||
                            managedKind === 'task-space' ||
                            element.classList.contains('omni-streaming-chunk')
                        ) {
                            return { type: 'text', value: element.textContent ?? '' }
                        }

                        const preserveChildWhitespace =
                            preserveWhitespace || element.tagName === 'PRE'
                        const children: NodeValue[] = []
                        for (const child of element.childNodes) {
                            appendValue(children, visit(child, preserveChildWhitespace))
                        }
                        if (managedKind === 'raw-html') return children

                        return {
                            type: 'element',
                            tag: element.tagName.toLowerCase(),
                            attributes: Array.from(element.attributes)
                                .filter(({ name }) => name !== 'data-omni-markdown-kind')
                                .sort((a, b) => a.name.localeCompare(b.name))
                                .map(({ name, value }) => `${name}=${value}`),
                            children,
                        }
                    }

                    const children: NodeValue[] = []
                    for (const child of root.childNodes) appendValue(children, visit(child))
                    return children
                }

                const staticRoot = document.createElement('div')
                staticRoot.innerHTML = window.StreamingMarkdownTest.renderMarkdown(markdown)
                const streamingRoot = document.createElement('div')
                const renderer = new window.StreamingMarkdownTest.StreamingMarkdownRenderer(
                    streamingRoot,
                )
                renderer.enqueue({ source: markdown, isStreaming: false })
                return {
                    static: normalize(staticRoot),
                    streaming: normalize(streamingRoot),
                }
            }, source)
            expect(parity.streaming).toEqual(parity.static)
        }

        const generatedAttributes = await page.evaluate(() => {
            const root = document.createElement('div')
            root.innerHTML = window.StreamingMarkdownTest.renderMarkdown(
                String.raw`[safe](<https://example.com/a b> "a\" onmouseover=\"alert(1)")

![safe\" onerror=\"alert(1)](<https://example.com/a b.png>)`,
            )
            const link = root.querySelector('a')
            const image = root.querySelector('img')
            return {
                linkHref: link?.getAttribute('href'),
                linkTitle: link?.getAttribute('title'),
                linkHasHandler: link?.hasAttribute('onmouseover'),
                imageSource: image?.getAttribute('src'),
                imageAlt: image?.getAttribute('alt'),
                imageHasHandler: image?.hasAttribute('onerror'),
            }
        })
        expect(generatedAttributes).toEqual({
            linkHref: 'https://example.com/a%20b',
            linkTitle: 'a" onmouseover="alert(1)',
            linkHasHandler: false,
            imageSource: 'https://example.com/a%20b.png',
            imageAlt: 'safe" onerror="alert(1)',
            imageHasHandler: false,
        })
    } finally {
        await rm(directory, { recursive: true, force: true })
    }
})
