import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

const pocDirectory = dirname(fileURLToPath(import.meta.url))
const webDirectory = resolve(pocDirectory, '..')
const productionRenderer = resolve(webDirectory, 'src/lib/markdown/streaming-markdown-renderer.ts')
const bundledRenderer = '/tmp/omni-streaming-markdown-renderer.js'
const esbuild = resolve(webDirectory, 'node_modules/vite/node_modules/esbuild/bin/esbuild')
execFileSync(esbuild, [
    productionRenderer,
    '--bundle',
    '--format=iife',
    '--global-name=StreamingMarkdownProduction',
    '--platform=browser',
    `--outfile=${bundledRenderer}`,
])

const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
const browser = await chromium.launch({
    headless: true,
    ...(executablePath ? { executablePath } : {}),
})
try {
    const page = await browser.newPage()
    await page.setContent(`
        <style>
            .omni-streaming-chunk {
                animation: poc-fade-in 400ms ease-out both;
            }
            @keyframes poc-fade-in {
                from { opacity: 0; }
                to { opacity: 1; }
            }
        </style>
        <main id="root"></main>
    `)
    await page.addScriptTag({ path: bundledRenderer })
    await page.evaluate(() => {
        const Renderer = window.StreamingMarkdownProduction.StreamingMarkdownRenderer
        window.poc = new Renderer(document.querySelector('#root'))
    })

    const update = async (source) => {
        await page.evaluate(async (markdown) => {
            window.poc.enqueue({ source: markdown, isStreaming: true })
            await new Promise(requestAnimationFrame)
        }, source)
    }

    await update('Hello')
    await page.evaluate(() => {
        window.initialParagraph = document.querySelector('#root > p')
        window.initialTextChunk = window.initialParagraph.querySelector(
            '[data-omni-markdown-kind="text"] > span',
        )
    })

    await update('Hello world')
    const firstAppend = await page.evaluate(() => {
        const paragraph = document.querySelector('#root > p')
        const textContainer = paragraph.querySelector('[data-omni-markdown-kind="text"]')
        const chunks = textContainer.children
        window.animatedWorldChunk = chunks[1]
        window.animatedWorldAnimation = window.animatedWorldChunk.getAnimations()[0]
        return {
            paragraphPreserved: paragraph === window.initialParagraph,
            initialChunkPreserved: chunks[0] === window.initialTextChunk,
            chunkTexts: Array.from(chunks, (chunk) => chunk.textContent),
            newChunkFades: chunks[1].classList.contains('omni-streaming-chunk'),
            animationCount: chunks[1].getAnimations().length,
        }
    })
    assert.equal(firstAppend.paragraphPreserved, true)
    assert.equal(firstAppend.initialChunkPreserved, true)
    assert.deepEqual(firstAppend.chunkTexts, ['Hello', ' world'])
    assert.equal(firstAppend.newChunkFades, true)
    assert.equal(firstAppend.animationCount, 1)

    await page.waitForTimeout(40)
    await update('Hello world again')
    const secondAppend = await page.evaluate(() => {
        const chunks = document.querySelector('#root > p [data-omni-markdown-kind="text"]').children
        return {
            paragraphPreserved: document.querySelector('#root > p') === window.initialParagraph,
            worldChunkPreserved: chunks[1] === window.animatedWorldChunk,
            worldAnimationPreserved: chunks[1].getAnimations()[0] === window.animatedWorldAnimation,
            chunkTexts: Array.from(chunks, (chunk) => chunk.textContent),
        }
    })
    assert.equal(secondAppend.paragraphPreserved, true)
    assert.equal(secondAppend.worldChunkPreserved, true)
    assert.equal(secondAppend.worldAnimationPreserved, true)
    assert.deepEqual(secondAppend.chunkTexts, ['Hello', ' world', ' again'])

    await update('- one')
    await page.evaluate(() => {
        window.initialList = document.querySelector('#root > ul')
        window.initialListItem = window.initialList.querySelector('li')
        window.initialListChunk = window.initialListItem.querySelector(
            '[data-omni-markdown-kind="text"]',
        )
    })
    await update('- one\n- two')
    const listAppend = await page.evaluate(() => {
        const list = document.querySelector('#root > ul')
        const items = list.querySelectorAll(':scope > li')
        return {
            listPreserved: list === window.initialList,
            firstItemPreserved: items[0] === window.initialListItem,
            firstChunkPreserved:
                items[0].querySelector('[data-omni-markdown-kind="text"]') ===
                window.initialListChunk,
            itemTexts: Array.from(items, (item) => item.textContent),
            secondItemFades: items[1]
                .querySelector('[data-omni-markdown-kind="text"] > span:last-child')
                .classList.contains('omni-streaming-chunk'),
        }
    })
    assert.equal(listAppend.listPreserved, true)
    assert.equal(listAppend.firstItemPreserved, true)
    assert.equal(listAppend.firstChunkPreserved, true)
    assert.deepEqual(listAppend.itemTexts, ['one', 'two'])
    assert.equal(listAppend.secondItemFades, true)

    await page.evaluate(() => {
        window.secondListItem = document.querySelectorAll('#root > ul > li')[1]
        window.secondListChunk = window.secondListItem.querySelector(
            '[data-omni-markdown-kind="text"] > span',
        )
    })
    await update('- one\n- two continued')
    const nestedTextAppend = await page.evaluate(() => {
        const secondItem = document.querySelectorAll('#root > ul > li')[1]
        const chunks = secondItem.querySelector('[data-omni-markdown-kind="text"]').children
        return {
            itemPreserved: secondItem === window.secondListItem,
            oldChunkPreserved: chunks[0] === window.secondListChunk,
            chunkTexts: Array.from(chunks, (chunk) => chunk.textContent),
        }
    })
    assert.equal(nestedTextAppend.itemPreserved, true)
    assert.equal(nestedTextAppend.oldChunkPreserved, true)
    assert.deepEqual(nestedTextAppend.chunkTexts, ['two', ' continued'])

    await update('| A | B |\n| - | - |')
    await page.evaluate(() => {
        window.initialTable = document.querySelector('#root > table')
        window.initialHeaderCell = window.initialTable.querySelector('th')
        window.initialHeaderChunk = window.initialHeaderCell.querySelector(
            '[data-omni-markdown-kind="text"]',
        )
    })
    await update('| A | B |\n| - | - |\n| 1 | 2 |')
    const tableAppend = await page.evaluate(() => {
        const table = document.querySelector('#root > table')
        return {
            tablePreserved: table === window.initialTable,
            headerPreserved: table.querySelector('th') === window.initialHeaderCell,
            headerChunkPreserved:
                table.querySelector('th [data-omni-markdown-kind="text"]') ===
                window.initialHeaderChunk,
            rowText: table.querySelector('tbody tr').textContent,
            rowFades: Array.from(
                table.querySelectorAll('tbody [data-omni-markdown-kind="text"] > span'),
                (chunk) => chunk.classList.contains('omni-streaming-chunk'),
            ),
        }
    })
    assert.equal(tableAppend.tablePreserved, true)
    assert.equal(tableAppend.headerPreserved, true)
    assert.equal(tableAppend.headerChunkPreserved, true)
    assert.equal(tableAppend.rowText, '12')
    assert.deepEqual(tableAppend.rowFades, [true, true])

    await update('Hello **bo')
    await page.evaluate(() => {
        window.preStrongParagraph = document.querySelector('#root > p')
    })
    await update('Hello **bold**')
    const structuralChange = await page.evaluate(() => {
        const paragraph = document.querySelector('#root > p')
        const strong = paragraph.querySelector('strong')
        return {
            paragraphPreserved: paragraph === window.preStrongParagraph,
            text: paragraph.textContent,
            strongText: strong?.textContent,
            strongChunkFades: strong
                ?.querySelector('.omni-streaming-chunk')
                ?.classList.contains('omni-streaming-chunk'),
        }
    })
    assert.equal(structuralChange.paragraphPreserved, true)
    assert.equal(structuralChange.text, 'Hello bold')
    assert.equal(structuralChange.strongText, 'bold')
    assert.equal(structuralChange.strongChunkFades, true)

    const corpus = [
        {
            source: '# Heading',
            check: () => ({ tag: document.querySelector('#root > h1')?.tagName }),
            expected: { tag: 'H1' },
        },
        {
            source: '> quoted text',
            check: () => ({
                tag: document.querySelector('#root > blockquote')?.tagName,
                text: document.querySelector('#root > blockquote')?.textContent,
            }),
            expected: { tag: 'BLOCKQUOTE', text: 'quoted text' },
        },
        {
            source: '- [x] done\n- not done',
            check: () => ({
                checked: document.querySelector('#root input')?.hasAttribute('checked'),
                itemCount: document.querySelectorAll('#root > ul > li').length,
            }),
            expected: { checked: true, itemCount: 2 },
        },
        {
            source: '```js\nconst value = 1\n```',
            check: () => ({
                language: document.querySelector('#root code')?.className,
                text: document.querySelector('#root code')?.textContent,
            }),
            expected: { language: 'language-js', text: 'const value = 1\n' },
        },
        {
            source: '[link](https://example.com)',
            check: () => ({
                href: document.querySelector('#root a')?.getAttribute('href'),
                target: document.querySelector('#root a')?.getAttribute('target'),
                rel: document.querySelector('#root a')?.getAttribute('rel'),
            }),
            expected: {
                href: 'https://example.com',
                target: '_blank',
                rel: 'noopener noreferrer',
            },
        },
        {
            source: '<div>raw html</div>',
            check: () => ({ text: document.querySelector('#root')?.textContent }),
            expected: { text: 'raw html' },
        },
    ]
    for (const entry of corpus) {
        await update(entry.source)
        assert.deepEqual(await page.evaluate(entry.check), entry.expected)
    }

    console.log('Streaming Markdown renderer passed: persistent nodes and animations verified.')
} finally {
    await browser.close()
}
