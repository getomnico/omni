import { describe, expect, it } from 'vitest'
import { filesFromClipboardData, imageUrlsFromClipboardHtml } from './clipboard'

function fakeDataTransfer({
    items = [],
    files = [],
}: {
    items?: Array<{ kind: string; getAsFile: () => File | null }>
    files?: File[]
}) {
    return {
        items,
        files,
    } as unknown as DataTransfer
}

describe('filesFromClipboardData', () => {
    it('collects file items', () => {
        const file = new File(['x'], 'a.png', { type: 'image/png' })
        const data = fakeDataTransfer({
            items: [
                { kind: 'string', getAsFile: () => null },
                { kind: 'file', getAsFile: () => file },
            ],
        })

        expect(filesFromClipboardData(data)).toEqual([file])
    })

    it('falls back to the files list when items are empty', () => {
        const file = new File(['x'], 'b.png', { type: 'image/png' })
        const data = fakeDataTransfer({ items: [], files: [file] })

        expect(filesFromClipboardData(data)).toEqual([file])
    })

    it('returns empty for null clipboard', () => {
        expect(filesFromClipboardData(null)).toEqual([])
    })
})

describe('imageUrlsFromClipboardHtml', () => {
    it('extracts quoted and unquoted srcs, preserving order and deduplicating', () => {
        const html =
            'before <img src="https://ex.invalid/a.png"> mid <img class="x" src=\'https://ex.invalid/b.jpg\'> <img src="https://ex.invalid/a.png">'

        expect(imageUrlsFromClipboardHtml(html)).toEqual([
            'https://ex.invalid/a.png',
            'https://ex.invalid/b.jpg',
        ])
    })

    it('ignores markup without images', () => {
        expect(imageUrlsFromClipboardHtml('<p>just text</p>')).toEqual([])
        expect(imageUrlsFromClipboardHtml(null)).toEqual([])
    })
})
