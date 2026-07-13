import type { TextCitationParam } from '@anthropic-ai/sdk/resources'
import { SourceType } from '$lib/types'
import { inferSourceFromUrl, getSourceDisplayName, parseUrlMetadata } from '$lib/utils/icons'

export type NormalizedCitation = {
    citationId: string
    sourceId: string
    title: string
    citedText: string
    href: string | null
    iconHint: string | null
    isImap: boolean
    sourceName: string
    locationLabel: string | null
}

function exhaustiveCheck(citation: never): never {
    throw new Error(`Unknown citation type: ${(citation as { type: string }).type ?? 'undefined'}`)
}

const syntheticCitationPattern = /\[citation:([\d,\s]+)\]/g

function citationPlaceholder(citation: TextCitationParam): string {
    return `{omni-cit:${encodeURIComponent(citationIdFromCitation(citation))}}`
}

/**
 * Replace preserved synthetic citation markers in their original positions.
 * Native citation blocks do not contain markers, so their citations remain
 * attached to the end of the block as before.
 */
export function placeCitationPlaceholders(text: string, citations: TextCitationParam[]): string {
    let citationIndex = 0
    const withInlineCitations = text.replace(syntheticCitationPattern, (marker, references) => {
        const referenceCount = references.match(/\d+/g)?.length ?? 0
        const markerCitations = citations.slice(citationIndex, citationIndex + referenceCount)
        citationIndex += referenceCount

        return markerCitations.length > 0
            ? markerCitations.map(citationPlaceholder).join('')
            : marker
    })

    const remainingPlaceholders = citations.slice(citationIndex).map(citationPlaceholder).join('')
    return remainingPlaceholders
        ? `${withInlineCitations} ${remainingPlaceholders}`
        : withInlineCitations
}

// ---------------------------------------------------------------------------
// Per-citation identity (presentation-exact, includes title + cited_text)
// ---------------------------------------------------------------------------

export function citationIdFromCitation(citation: TextCitationParam): string {
    switch (citation.type) {
        case 'search_result_location':
            return JSON.stringify([
                citation.type,
                citation.source,
                citation.search_result_index,
                citation.start_block_index,
                citation.end_block_index,
                citation.title,
                citation.cited_text,
            ])
        case 'web_search_result_location':
            return JSON.stringify([
                citation.type,
                citation.url,
                citation.encrypted_index,
                citation.title,
                citation.cited_text,
            ])
        case 'char_location':
            return JSON.stringify([
                citation.type,
                citation.document_index,
                citation.document_title,
                citation.start_char_index,
                citation.end_char_index,
                citation.cited_text,
            ])
        case 'page_location':
            return JSON.stringify([
                citation.type,
                citation.document_index,
                citation.document_title,
                citation.start_page_number,
                citation.end_page_number,
                citation.cited_text,
            ])
        case 'content_block_location':
            return JSON.stringify([
                citation.type,
                citation.document_index,
                citation.document_title,
                citation.start_block_index,
                citation.end_block_index,
                citation.cited_text,
            ])
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Source identity (drawer deduplication)
// ---------------------------------------------------------------------------

export function sourceIdentityFromCitation(citation: TextCitationParam): string {
    switch (citation.type) {
        case 'search_result_location':
            return JSON.stringify(['source', citation.source])
        case 'web_search_result_location':
            return JSON.stringify(['source', citation.url])
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return JSON.stringify(['document', citation.document_index])
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Source display name
// ---------------------------------------------------------------------------

function sourceNameFromUrl(url: string): string | null {
    const metadataSourceType = parseUrlMetadata(url).sourceType
    if (metadataSourceType) {
        const metadataName = getSourceDisplayName(metadataSourceType as SourceType)
        if (metadataName) return metadataName
    }

    const inferredSourceType = inferSourceFromUrl(url)
    return inferredSourceType ? (getSourceDisplayName(inferredSourceType) ?? null) : null
}

function sourceDisplayName(citation: TextCitationParam): string {
    switch (citation.type) {
        case 'search_result_location': {
            const source = citation.source
            if (source.startsWith('imap:')) {
                return getSourceDisplayName(SourceType.IMAP) ?? 'IMAP'
            }
            if (source.startsWith('http://') || source.startsWith('https://')) {
                return sourceNameFromUrl(source) ?? 'Web'
            }
            return 'Files'
        }
        case 'web_search_result_location':
            return sourceNameFromUrl(citation.url) ?? 'Web'
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return 'Files'
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Fallback title
// ---------------------------------------------------------------------------

function fallbackTitle(citation: TextCitationParam): string {
    switch (citation.type) {
        case 'search_result_location':
            return citation.title ?? citation.source
        case 'web_search_result_location':
            return citation.title ?? citation.url
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return citation.document_title ?? `Document ${citation.document_index + 1}`
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Location label
// ---------------------------------------------------------------------------

function locationLabel(citation: TextCitationParam): string | null {
    switch (citation.type) {
        case 'search_result_location':
            return null
        case 'web_search_result_location':
            return null
        case 'char_location':
            return 'Document excerpt'
        case 'page_location':
            if (citation.start_page_number === citation.end_page_number) {
                return `Page ${citation.start_page_number}`
            }
            return `Pages ${citation.start_page_number}–${citation.end_page_number}`
        case 'content_block_location':
            return 'Document excerpt'
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Icon hint
// ---------------------------------------------------------------------------

function iconHintFromCitation(citation: TextCitationParam): string | null {
    switch (citation.type) {
        case 'search_result_location':
            return citation.source ?? null
        case 'web_search_result_location':
            return citation.url ?? null
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return null
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Navigable href
// ---------------------------------------------------------------------------

function navigableHref(citation: TextCitationParam): string | null {
    switch (citation.type) {
        case 'search_result_location':
            if (citation.source.startsWith('http://') || citation.source.startsWith('https://')) {
                return citation.source
            }
            return null
        case 'web_search_result_location':
            if (citation.url.startsWith('http://') || citation.url.startsWith('https://')) {
                return citation.url
            }
            return null
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return null
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// IMAP check
// ---------------------------------------------------------------------------

function isImapFromCitation(citation: TextCitationParam): boolean {
    switch (citation.type) {
        case 'search_result_location':
            return citation.source?.startsWith('imap:') === true
        case 'web_search_result_location':
        case 'char_location':
        case 'page_location':
        case 'content_block_location':
            return false
    }
    return exhaustiveCheck(citation)
}

// ---------------------------------------------------------------------------
// Public normalizer
// ---------------------------------------------------------------------------

export function normalizeCitation(citation: TextCitationParam): NormalizedCitation {
    return {
        citationId: citationIdFromCitation(citation),
        sourceId: sourceIdentityFromCitation(citation),
        title: fallbackTitle(citation),
        citedText: citation.cited_text,
        sourceName: sourceDisplayName(citation),
        href: navigableHref(citation),
        iconHint: iconHintFromCitation(citation),
        isImap: isImapFromCitation(citation),
        locationLabel: locationLabel(citation),
    }
}
