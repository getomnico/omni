import type {
    ContentBlockParam,
    DocumentBlockParam,
    MessageParam,
    SearchResultBlockParam,
    TextCitationParam,
    ToolResultBlockParam,
} from '@anthropic-ai/sdk/resources/messages'
import type { OmniMentionBlock } from '$lib/types/message'

const READ_DOCUMENT_TOOL_NAME = 'read_document'
const REF_PREFIX = '_ref:'
const REF_MARKER_PATTERN = /^\[_ref:([^\]\s]+)\]$/

/** Search results carry Omni's internal document_id next to the SDK fields. */
export type OmniSearchResultBlockParam = SearchResultBlockParam & {
    document_id?: string
}

/**
 * Maps citation identities to indexed document IDs. A `null` value marks an
 * identity that matched more than one document, so it cannot be resolved safely.
 */
export type CitationDocumentIndex = ReadonlyMap<string, string | null>

type ToolResultContentItem = Exclude<ToolResultBlockParam['content'], string | undefined>[number]

function searchResultKey(source: string, title: string | null): string {
    return JSON.stringify(['search_result', source, title])
}

function documentKey(title: string): string {
    return JSON.stringify(['document', title])
}

function addEntry(index: Map<string, string | null>, key: string, documentId: string) {
    if (!index.has(key)) {
        index.set(key, documentId)
    } else if (index.get(key) !== documentId) {
        index.set(key, null)
    }
}

function stripRefPrefix(documentId: string): string {
    return documentId.startsWith(REF_PREFIX) ? documentId.slice(REF_PREFIX.length) : documentId
}

function readDocumentIdFromInput(input: unknown): string | null {
    if (typeof input !== 'object' || input === null || !('id' in input)) return null
    const id = input.id
    if (typeof id !== 'string') return null
    const documentId = stripRefPrefix(id.trim())
    return documentId.length > 0 ? documentId : null
}

function searchResultDocumentId(block: OmniSearchResultBlockParam): string | null {
    if (typeof block.document_id === 'string' && block.document_id.length > 0) {
        return block.document_id
    }
    // Results persisted before document_id existed only carry the ID in the
    // [_ref:ULID] marker the AI service adds for the read_document tool.
    for (const item of block.content) {
        if (item.type !== 'text') continue
        const match = REF_MARKER_PATTERN.exec(item.text)
        if (match) return match[1]
    }
    return null
}

function isSearchResultBlock(item: ToolResultContentItem): item is OmniSearchResultBlockParam {
    return item.type === 'search_result'
}

function isDocumentBlock(item: ToolResultContentItem): item is DocumentBlockParam {
    return item.type === 'document'
}

// Mentions are stored unexpanded; the AI service expands each into a document
// block titled after the mentioned document before calling the model.
function asMentionBlock(block: ContentBlockParam): OmniMentionBlock | null {
    if (block.type !== 'document') return null
    const source: unknown = block.source
    if (typeof source !== 'object' || source === null) return null
    if (!('type' in source) || source.type !== 'omni_mention') return null
    if (!('document_id' in source) || typeof source.document_id !== 'string') return null
    if (!('title' in source) || typeof source.title !== 'string') return null
    return {
        type: 'document',
        source: { type: 'omni_mention', document_id: source.document_id, title: source.title },
    }
}

function contentBlocks(message: MessageParam): ContentBlockParam[] {
    return typeof message.content === 'string' ? [] : message.content
}

/**
 * Builds the citation-to-document index from the raw messages of a chat path.
 *
 * Citations only carry presentation fields (title/source), so they are matched
 * against the blocks that produced them: search results by source + title,
 * read_document results and mentioned documents by document title.
 */
export function buildCitationDocumentIndex(messages: MessageParam[]): CitationDocumentIndex {
    const readDocumentIdsByToolUseId = new Map<string, string>()
    for (const message of messages) {
        for (const block of contentBlocks(message)) {
            if (block.type !== 'tool_use' || block.name !== READ_DOCUMENT_TOOL_NAME) continue
            const documentId = readDocumentIdFromInput(block.input)
            if (documentId) readDocumentIdsByToolUseId.set(block.id, documentId)
        }
    }

    const index = new Map<string, string | null>()
    for (const message of messages) {
        for (const block of contentBlocks(message)) {
            const mention = asMentionBlock(block)
            if (mention) {
                addEntry(index, documentKey(mention.source.title), mention.source.document_id)
                continue
            }
            if (block.type !== 'tool_result' || !Array.isArray(block.content)) continue
            const readDocumentId = readDocumentIdsByToolUseId.get(block.tool_use_id) ?? null

            for (const item of block.content) {
                if (isSearchResultBlock(item)) {
                    const documentId = searchResultDocumentId(item)
                    if (documentId) {
                        addEntry(index, searchResultKey(item.source, item.title), documentId)
                    }
                } else if (isDocumentBlock(item) && readDocumentId && item.title) {
                    addEntry(index, documentKey(item.title), readDocumentId)
                }
            }
        }
    }
    return index
}

export function resolveCitationDocumentId(
    index: CitationDocumentIndex,
    citation: TextCitationParam,
): string | null {
    switch (citation.type) {
        case 'search_result_location':
            return index.get(searchResultKey(citation.source, citation.title)) ?? null
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            if (!citation.document_title) return null
            return index.get(documentKey(citation.document_title)) ?? null
        case 'web_search_result_location':
            return null
    }
}
