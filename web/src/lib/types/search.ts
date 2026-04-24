export interface DocumentMetadata {
    title?: string
    author?: string
    created_at?: string
    updated_at?: string
    mime_type?: string
    size?: string
    url?: string
    path?: string
    extra?: Record<string, any>
}

export interface GmailExtra {
    thread_id?: string
    participants?: string[]
}

export interface SlackExtra {
    slack?: {
        channel_id?: string
        message_count?: number
        authors?: string[]
        date?: string
        thread_ts?: string
    }
}

export interface GoogleDriveExtra {
    file_id?: string
    shared?: boolean
    google_drive?: { parents?: string[]; parent_id?: string }
}

export interface ConfluenceExtra {
    confluence?: { parent_id?: string; version?: number }
}

export interface JiraExtra {
    jira?: { project_id?: string }
}

export interface JiraAttributes {
    issue_key?: string
    issue_type?: string
    status?: string
    status_category?: string
    project_key?: string
    project_name?: string
    priority?: string
    assignee?: string
    reporter?: string
    labels?: string[]
    components?: string[]
}

export interface ConfluenceAttributes {
    space_id?: string
    status?: string
}

export interface Document {
    id: string
    title: string
    url: string | null
    source_id: string
    content_type: string
    created_at: string
    updated_at: string
    metadata?: DocumentMetadata
    attributes?: Record<string, any>
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
    count?: number
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
    active_filters?: Facet[]
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

export interface RecentSearchesResponse {
    searches: string[]
}

export interface SuggestedQuestion {
    question: string
    document_id: string
}

export interface SuggestedQuestionsResponse {
    questions: SuggestedQuestion[]
}

export interface TypeaheadResult {
    document_id: string
    title: string
    url: string | null
    source_id: string
}

export interface TypeaheadResponse {
    results: TypeaheadResult[]
    query: string
}

// ============================================================================
// Connector Action Types
// ============================================================================

export interface GoogleDirectoryUser {
    id: string
    email: string
    name: string
    orgUnit: string
    suspended: boolean
    isAdmin: boolean
}

export interface SearchUsersResponse {
    users: GoogleDirectoryUser[]
    nextPageToken: string | null
    hasMore: boolean
}

export interface ConnectorActionResponse<T = unknown> {
    status: string
    result?: T
    error?: string
}
