import type { ProcessedMessage, ToolMessageContent } from '$lib/types/message'

// How an artifact is presented in the chat UI. Hard-coded by content type:
// images render inline in the message stream, everything else opens in the
// right-hand artifact pane.
export type ArtifactDisplayMode = 'inline' | 'panel'

export type ArtifactKind =
    | 'image'
    | 'pdf'
    | 'html'
    | 'docx'
    | 'xlsx'
    | 'markdown'
    | 'text'
    | 'other'

export type ArtifactData = {
    key: string
    url: string
    title: string
    content_type: string
    size_bytes: number
}

export type ParsedArtifact = Omit<ArtifactData, 'key'>

// Preview size guards: conversions run in the browser, so cap the inputs.
export const MAX_DOCX_PREVIEW_BYTES = 25 * 1024 * 1024
export const MAX_XLSX_PREVIEW_BYTES = 25 * 1024 * 1024
export const MAX_TEXT_PREVIEW_BYTES = 1024 * 1024

const MIME_KINDS: Readonly<Record<string, ArtifactKind>> = {
    'application/pdf': 'pdf',
    'text/html': 'html',
    'application/vnd.openxmlformats-officedocument.wordprocessingml.document': 'docx',
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': 'xlsx',
}

// Non-text/* mimes that still preview fine in the plain-text viewer.
const TEXT_MIMES: ReadonlySet<string> = new Set([
    'application/json',
    'application/xml',
    'application/x-sh',
    'application/javascript',
    'application/x-yaml',
])

const MARKDOWN_MIMES: ReadonlySet<string> = new Set(['text/markdown', 'text/x-markdown'])

const EXT_KINDS: Readonly<Record<string, ArtifactKind>> = {
    pdf: 'pdf',
    html: 'html',
    htm: 'html',
    docx: 'docx',
    xlsx: 'xlsx',
    md: 'markdown',
    markdown: 'markdown',
    txt: 'text',
    csv: 'text',
    json: 'text',
    log: 'text',
}

export function parseArtifactResult(text: string): ParsedArtifact | null {
    let parsed: unknown
    try {
        parsed = JSON.parse(text)
    } catch {
        return null
    }
    if (typeof parsed !== 'object' || parsed === null) return null
    const candidate = parsed as Record<string, unknown>
    if (
        typeof candidate.url !== 'string' ||
        typeof candidate.title !== 'string' ||
        typeof candidate.content_type !== 'string' ||
        typeof candidate.size_bytes !== 'number'
    ) {
        return null
    }
    // Artifacts are always served through the same-origin chat proxy.
    if (!candidate.url.startsWith('/api/chat/')) return null
    return {
        url: candidate.url,
        title: candidate.title,
        content_type: candidate.content_type,
        size_bytes: candidate.size_bytes,
    }
}

export function fileNameFromArtifactUrl(url: string): string | null {
    const withoutQuery = url.split('?')[0]
    const segment = withoutQuery.split('/').pop()
    if (!segment) return null
    try {
        return decodeURIComponent(segment)
    } catch {
        return null
    }
}

export function artifactKind(
    contentType: string | null | undefined,
    url?: string | null,
): ArtifactKind {
    const mime = (contentType ?? '').toLowerCase()
    if (mime.startsWith('image/')) return 'image'
    if (mime in MIME_KINDS) return MIME_KINDS[mime]
    if (MARKDOWN_MIMES.has(mime)) return 'markdown'
    if (mime.startsWith('text/') || TEXT_MIMES.has(mime)) return 'text'

    // Content-type can degrade to octet-stream on some hops; fall back to the
    // file extension so known types still get the right viewer.
    if (!mime || mime === 'application/octet-stream') {
        const name = url ? fileNameFromArtifactUrl(url) : null
        if (name) {
            const dot = name.lastIndexOf('.')
            if (dot !== -1 && dot < name.length - 1) {
                const ext = name.slice(dot + 1).toLowerCase()
                if (ext in EXT_KINDS) return EXT_KINDS[ext]
            }
        }
    }
    return 'other'
}

export function artifactDisplayMode(
    contentType: string | null | undefined,
    url?: string | null,
): ArtifactDisplayMode {
    return artifactKind(contentType, url) === 'image' ? 'inline' : 'panel'
}

export function artifactKindLabel(kind: ArtifactKind): string {
    switch (kind) {
        case 'image':
            return 'Image'
        case 'pdf':
            return 'PDF document'
        case 'html':
            return 'HTML page'
        case 'docx':
            return 'Word document'
        case 'xlsx':
            return 'Excel workbook'
        case 'markdown':
            return 'Markdown'
        case 'text':
            return 'Text file'
        default:
            return 'File'
    }
}

export function formatArtifactSize(sizeBytes: number): string {
    if (!Number.isFinite(sizeBytes) || sizeBytes < 0) return '0 B'
    if (sizeBytes < 1024) return `${sizeBytes} B`
    if (sizeBytes < 1024 * 1024) return `${Math.round(sizeBytes / 1024)} KB`
    return `${(sizeBytes / (1024 * 1024)).toFixed(1)} MB`
}

export function escapeHtml(value: string): string {
    return value
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;')
        .replaceAll('"', '&quot;')
        .replaceAll("'", '&#39;')
}

// Parses the JSON tool result emitted by `present_artifact` into an artifact.
export function artifactFromToolCall(message: ToolMessageContent): ArtifactData | null {
    if (message.toolUse.name !== 'present_artifact') return null
    const text = message.actionResult?.text
    if (!text) return null
    const parsed = parseArtifactResult(text)
    if (!parsed) return null
    return { ...parsed, key: message.toolUse.id }
}

// Panel-mode artifacts across the whole conversation. The chat page derives its
// artifact pane state from this, so chips and pane share one source of truth.
export function collectPanelArtifacts(messages: readonly ProcessedMessage[]): ArtifactData[] {
    const artifacts: ArtifactData[] = []
    for (const message of messages) {
        if (message.role !== 'assistant') continue
        for (const block of message.content) {
            if (block.type !== 'tool') continue
            const artifact = artifactFromToolCall(block)
            if (artifact && artifactDisplayMode(artifact.content_type, artifact.url) === 'panel') {
                artifacts.push(artifact)
            }
        }
    }
    return artifacts
}
