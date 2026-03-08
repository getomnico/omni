export enum SourceType {
    GOOGLE_DRIVE = 'google_drive',
    GMAIL = 'gmail',
    CONFLUENCE = 'confluence',
    JIRA = 'jira',
    SLACK = 'slack',
    GITHUB = 'github',
    LOCAL_FILES = 'local_files',
    WEB = 'web',
    NOTION = 'notion',
    HUBSPOT = 'hubspot',
    ONE_DRIVE = 'one_drive',
    SHARE_POINT = 'share_point',
    OUTLOOK = 'outlook',
    OUTLOOK_CALENDAR = 'outlook_calendar',
    FIREFLIES = 'fireflies',
    IMAP = 'imap',
}

export enum ServiceProvider {
    GOOGLE = 'google',
    SLACK = 'slack',
    ATLASSIAN = 'atlassian',
    GITHUB = 'github',
    MICROSOFT = 'microsoft',
    HUBSPOT = 'hubspot',
    FIREFLIES = 'fireflies',
    IMAP = 'imap',
}

export enum AuthType {
    JWT = 'jwt',
    API_KEY = 'api_key',
    BASIC_AUTH = 'basic_auth',
    BEARER_TOKEN = 'bearer_token',
    BOT_TOKEN = 'bot_token',
    OAUTH = 'oauth',
}

export interface WebSourceConfig {
    root_url: string
    max_depth: number
    max_pages: number
    respect_robots_txt: boolean
    include_subdomains: boolean
    blacklist_patterns: string[]
    user_agent: string | null
}

export interface ConfluenceSourceConfig {
    base_url: string
    space_filters?: string[]
}

export interface JiraSourceConfig {
    base_url: string
    project_filters?: string[]
}

export interface GoogleDriveSourceConfig {
    // Future: shared_drive_filters, mime_type_filters, folder_path_filters, etc.
}

export interface GmailSourceConfig {
    // Future: label_filters, date_range_filters, etc.
}

export interface FilesystemSourceConfig {
    base_path: string
    file_extensions?: string[]
    exclude_patterns?: string[]
    max_file_size_bytes?: number
    scan_interval_seconds?: number
}

export interface HubspotSourceConfig {
    portal_id?: string
}

export interface ImapSourceConfig {
    display_name?: string
    host: string
    port: number
    /** "tls" | "starttls" | "none" */
    encryption: string
    folder_allowlist: string[]
    folder_denylist: string[]
    /** 0 = unlimited */
    max_message_size: number
    sync_enabled: boolean
}

export const DEFAULT_SYNC_INTERVAL_SECONDS: Record<SourceType, number> = {
    [SourceType.GOOGLE_DRIVE]: 1800,
    [SourceType.GMAIL]: 1800,
    [SourceType.SLACK]: 1800,
    [SourceType.OUTLOOK]: 1800,
    [SourceType.ONE_DRIVE]: 1800,
    [SourceType.CONFLUENCE]: 3600,
    [SourceType.JIRA]: 3600,
    [SourceType.GITHUB]: 3600,
    [SourceType.NOTION]: 3600,
    [SourceType.HUBSPOT]: 3600,
    [SourceType.SHARE_POINT]: 3600,
    [SourceType.OUTLOOK_CALENDAR]: 3600,
    [SourceType.FIREFLIES]: 3600,
    [SourceType.IMAP]: 3600,
    [SourceType.LOCAL_FILES]: 86400,
    [SourceType.WEB]: 86400,
}

export const EMBEDDING_PROVIDER_TYPES = ['local', 'jina', 'openai', 'cohere', 'bedrock'] as const
export type EmbeddingProviderType = (typeof EMBEDDING_PROVIDER_TYPES)[number]

export const PROVIDER_LABELS: Record<EmbeddingProviderType, string> = {
    local: 'Local (vLLM)',
    jina: 'Jina AI',
    openai: 'OpenAI',
    cohere: 'Cohere',
    bedrock: 'AWS Bedrock',
}
