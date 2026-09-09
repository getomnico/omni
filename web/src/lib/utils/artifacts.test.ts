import { describe, expect, it } from 'vitest'
import type { ProcessedMessage, ToolMessageContent } from '$lib/types/message'
import {
    artifactDisplayMode,
    artifactFromToolCall,
    artifactKind,
    artifactKindLabel,
    collectPanelArtifacts,
    escapeHtml,
    fileNameFromArtifactUrl,
    formatArtifactSize,
    parseArtifactResult,
} from './artifacts'

const PDF_RESULT = JSON.stringify({
    url: '/api/chat/chat-1/artifacts/report.pdf',
    title: 'Quarterly Report',
    content_type: 'application/pdf',
    size_bytes: 2048,
})

function toolMessage(overrides: Partial<ToolMessageContent> = {}): ToolMessageContent {
    return {
        id: 0,
        type: 'tool',
        status: 'completed',
        toolUse: { id: 'tool-1', name: 'present_artifact', input: {} },
        ...overrides,
    }
}

function assistantMessage(content: ToolMessageContent[]): ProcessedMessage {
    return {
        id: 1,
        sourceMessageIds: ['m1'],
        renderKey: 'r1',
        origMessageId: 'm1',
        role: 'assistant',
        content,
    }
}

describe('parseArtifactResult', () => {
    it('parses a well-formed present_artifact result', () => {
        expect(parseArtifactResult(PDF_RESULT)).toEqual({
            url: '/api/chat/chat-1/artifacts/report.pdf',
            title: 'Quarterly Report',
            content_type: 'application/pdf',
            size_bytes: 2048,
        })
    })

    it('rejects invalid JSON and malformed payloads', () => {
        expect(parseArtifactResult('not json')).toBeNull()
        expect(parseArtifactResult('{"url": 1}')).toBeNull()
        expect(
            parseArtifactResult(
                JSON.stringify({
                    url: 'https://evil.example/x',
                    title: 'x',
                    content_type: 'text/html',
                    size_bytes: 1,
                }),
            ),
        ).toBeNull()
        expect(parseArtifactResult('')).toBeNull()
    })
})

describe('artifactKind', () => {
    it('maps mime types to kinds', () => {
        expect(artifactKind('image/png')).toBe('image')
        expect(artifactKind('image/svg+xml')).toBe('image')
        expect(artifactKind('application/pdf')).toBe('pdf')
        expect(artifactKind('text/html')).toBe('html')
        expect(artifactKind('text/plain')).toBe('text')
        expect(artifactKind('text/csv')).toBe('text')
        expect(artifactKind('text/markdown')).toBe('markdown')
        expect(artifactKind('text/x-markdown')).toBe('markdown')
        expect(artifactKind('application/json')).toBe('text')
        expect(artifactKind('application/xml')).toBe('text')
        expect(
            artifactKind('application/vnd.openxmlformats-officedocument.wordprocessingml.document'),
        ).toBe('docx')
        expect(
            artifactKind('application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'),
        ).toBe('xlsx')
        expect(artifactKind('application/zip')).toBe('other')
    })

    it('falls back to the file extension for octet-stream', () => {
        expect(artifactKind('application/octet-stream', '/api/chat/c/artifacts/a.pdf')).toBe('pdf')
        expect(artifactKind('application/octet-stream', '/api/chat/c/artifacts/report.xlsx')).toBe(
            'xlsx',
        )
        expect(artifactKind('', '/api/chat/c/artifacts/notes.docx')).toBe('docx')
        expect(artifactKind('application/octet-stream', '/api/chat/c/artifacts/readme.md')).toBe(
            'markdown',
        )
        expect(
            artifactKind('application/octet-stream', '/api/chat/c/artifacts/CHANGELOG.markdown'),
        ).toBe('markdown')
        expect(artifactKind('application/octet-stream', '/api/chat/c/artifacts/blob.xyz')).toBe(
            'other',
        )
    })

    it('is case-insensitive on mime and extension', () => {
        expect(artifactKind('Application/PDF')).toBe('pdf')
        expect(artifactKind('application/octet-stream', '/api/chat/c/artifacts/a.PDF')).toBe('pdf')
    })
})

describe('artifactDisplayMode', () => {
    it('renders images inline and everything else in the panel', () => {
        expect(artifactDisplayMode('image/png')).toBe('inline')
        expect(artifactDisplayMode('application/pdf')).toBe('panel')
        expect(artifactDisplayMode('text/html')).toBe('panel')
        expect(artifactDisplayMode('application/octet-stream', '/x/y.unknown')).toBe('panel')
    })
})

describe('artifactKindLabel / formatArtifactSize', () => {
    it('labels known kinds', () => {
        expect(artifactKindLabel('pdf')).toBe('PDF document')
        expect(artifactKindLabel('xlsx')).toBe('Excel workbook')
        expect(artifactKindLabel('other')).toBe('File')
    })

    it('formats sizes', () => {
        expect(formatArtifactSize(500)).toBe('500 B')
        expect(formatArtifactSize(2048)).toBe('2 KB')
        expect(formatArtifactSize(3 * 1024 * 1024)).toBe('3.0 MB')
        expect(formatArtifactSize(-1)).toBe('0 B')
    })
})

describe('fileNameFromArtifactUrl', () => {
    it('extracts the last path segment', () => {
        expect(fileNameFromArtifactUrl('/api/chat/c/artifacts/a b.pdf')).toBe('a b.pdf')
        expect(fileNameFromArtifactUrl('/api/chat/c/artifacts/a.pdf?x=1')).toBe('a.pdf')
    })
})

describe('escapeHtml', () => {
    it('escapes markup characters', () => {
        expect(escapeHtml(`<a href="x">&'`)).toBe('&lt;a href=&quot;x&quot;&gt;&amp;&#39;')
    })
})

describe('artifactFromToolCall', () => {
    it('extracts artifacts only from present_artifact results', () => {
        const message = toolMessage({
            actionResult: { toolUseId: 'tool-1', text: PDF_RESULT },
        })
        expect(artifactFromToolCall(message)).toEqual({
            key: 'tool-1',
            url: '/api/chat/chat-1/artifacts/report.pdf',
            title: 'Quarterly Report',
            content_type: 'application/pdf',
            size_bytes: 2048,
        })
        expect(artifactFromToolCall(toolMessage({}))).toBeNull()
        expect(
            artifactFromToolCall(
                toolMessage({
                    toolUse: { id: 't', name: 'run_python', input: {} },
                    actionResult: { toolUseId: 't', text: PDF_RESULT },
                }),
            ),
        ).toBeNull()
    })
})

describe('collectPanelArtifacts', () => {
    it('collects panel artifacts and skips images and other tools', () => {
        const messages: ProcessedMessage[] = [
            assistantMessage([
                toolMessage({
                    toolUse: { id: 't-img', name: 'present_artifact', input: {} },
                    actionResult: {
                        toolUseId: 't-img',
                        text: JSON.stringify({
                            url: '/api/chat/c/artifacts/chart.png',
                            title: 'Chart',
                            content_type: 'image/png',
                            size_bytes: 100,
                        }),
                    },
                }),
            ]),
            assistantMessage([
                toolMessage({
                    toolUse: { id: 't-pdf', name: 'present_artifact', input: {} },
                    actionResult: { toolUseId: 't-pdf', text: PDF_RESULT },
                }),
                toolMessage({
                    toolUse: { id: 't-run', name: 'run_python', input: {} },
                }),
            ]),
            { ...assistantMessage([]), role: 'user' },
        ]

        const collected = collectPanelArtifacts(messages)
        expect(collected).toHaveLength(1)
        expect(collected[0]?.key).toBe('t-pdf')
    })
})
