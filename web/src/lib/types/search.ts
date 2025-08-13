export interface Document {
    id: string
    title: string
    url: string | null
    source_id: string
    content_type: string
    created_at: string
    updated_at: string
}

export interface SearchResult {
    document: Document
    score: number
    highlights: string[]
    match_type: string
    content?: string
}

export interface FacetValue {
    value: string
    count: number
}

export interface Facet {
    name: string
    values: FacetValue[]
}

export interface SearchResponse {
    results: SearchResult[]
    total_count: number
    query_time_ms: number
    has_more: boolean
    query: string
    facets?: Facet[]
}

export interface SearchRequest {
    query: string
    source_types?: string[]
    content_types?: string[]
    limit?: number
    offset?: number
    mode?: 'fulltext' | 'semantic' | 'hybrid'
    user_id?: string
}

export interface SuggestionsResponse {
    suggestions: string[]
    query: string
}

export interface RecentSearchesResponse {
    searches: string[]
}
