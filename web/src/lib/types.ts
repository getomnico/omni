export enum SourceType {
    GOOGLE_DRIVE = 'google_drive',
    GMAIL = 'gmail',
    GOOGLE_CHAT = 'google_chat',
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
    MS_TEAMS = 'ms_teams',
    FIREFLIES = 'fireflies',
    IMAP = 'imap',
    CLICKUP = 'clickup',
    LINEAR = 'linear',
    PAPERLESS_NGX = 'paperless_ngx',
    NEXTCLOUD = 'nextcloud',
    GOOGLE_ADS = 'google_ads',
    DARWINBOX = 'darwinbox',
    WINDSHIFT = 'windshift',
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
    CLICKUP = 'clickup',
    LINEAR = 'linear',
    NOTION = 'notion',
    PAPERLESS_NGX = 'paperless_ngx',
    NEXTCLOUD = 'nextcloud',
    GOOGLE_ADS = 'google_ads',
    DARWINBOX = 'darwinbox',
    WINDSHIFT = 'windshift',
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
    space_filters?: string[]
}

export interface JiraSourceConfig {
    project_filters?: string[]
}

export interface ClickUpSourceConfig {
    space_filters?: string[]
}

export interface GoogleDriveSourceConfig {
    // Future: shared_drive_filters, mime_type_filters, folder_path_filters, etc.
}

export interface GmailSourceConfig {
    // Future: label_filters, date_range_filters, etc.
}

export interface GoogleChatSourceConfig {
    space_allowlist?: string[]
    include_group_chats?: boolean
}

export interface FilesystemSourceConfig {
    base_path: string
    file_extensions?: string[]
    exclude_patterns?: string[]
    max_file_size_bytes?: number
}

export interface HubspotSourceConfig {
    portal_id?: string
}

export interface LinearSourceConfig {
    team_keys?: string[]
}

export interface GitHubSourceConfig {
    api_url?: string
    include_discussions?: boolean
    include_forks?: boolean
    repos?: string[]
    orgs?: string[]
    users?: string[]
    read_only?: boolean
}

export interface GitHubCredentials {
    token: string
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

export interface PaperlessNgxSourceConfig {
    base_url: string
    sync_enabled: boolean
}

export interface NextcloudSourceConfig {
    server_url: string
    base_path: string
    extension_allowlist: string[]
    extension_denylist: string[]
    /** 0 = unlimited */
    max_file_size: number
    sync_enabled: boolean
}

export interface GoogleAdsSourceConfig {
    customer_ids: string[]
    login_customer_id?: string
    entity_types?: string[]
    sync_enabled?: boolean
}

export interface DarwinboxSourceConfig {
    base_url: string
    default_timezone?: string
    sync_modules?: {
        employee_directory?: boolean
        deleted_employees?: boolean
        org_masters?: boolean
        positions?: boolean
        holidays?: boolean
        ats_jobs?: boolean
    }
    action_modules?: {
        employee_self_service?: boolean
        manager_workflows?: boolean
        hr_operations?: boolean
        ats?: boolean
        reports?: boolean
    }
    authorization?: {
        use_darwinbox_permissions?: boolean
        hr_admin_emails?: string[]
        recruiter_emails?: string[]
    }
}

export interface WindshiftSourceConfig {
    workspace_keys?: string[]
}

export interface WindshiftCredentials {
    access_token: string
    refresh_token?: string
}

export const DEFAULT_SYNC_INTERVAL_SECONDS: Record<SourceType, number> = {
    [SourceType.GOOGLE_DRIVE]: 1800,
    [SourceType.GMAIL]: 1800,
    [SourceType.GOOGLE_CHAT]: 1800,
    [SourceType.SLACK]: 1800,
    [SourceType.OUTLOOK]: 1800,
    [SourceType.ONE_DRIVE]: 1800,
    [SourceType.MS_TEAMS]: 1800,
    [SourceType.CONFLUENCE]: 3600,
    [SourceType.JIRA]: 3600,
    [SourceType.GITHUB]: 3600,
    [SourceType.NOTION]: 3600,
    [SourceType.HUBSPOT]: 3600,
    [SourceType.SHARE_POINT]: 3600,
    [SourceType.OUTLOOK_CALENDAR]: 3600,
    [SourceType.FIREFLIES]: 3600,
    [SourceType.IMAP]: 3600,
    [SourceType.LINEAR]: 3600,
    [SourceType.LOCAL_FILES]: 86400,
    [SourceType.WEB]: 86400,
    [SourceType.CLICKUP]: 3600,
    [SourceType.PAPERLESS_NGX]: 3600,
    [SourceType.NEXTCLOUD]: 3600,
    [SourceType.GOOGLE_ADS]: 3600,
    [SourceType.DARWINBOX]: 3600,
    [SourceType.WINDSHIFT]: 1800,
}

export const EMBEDDING_PROVIDER_TYPES = ['local', 'jina', 'openai', 'cohere', 'bedrock'] as const
export type EmbeddingProviderType = (typeof EMBEDDING_PROVIDER_TYPES)[number]

export const PROVIDER_LABELS: Record<EmbeddingProviderType, string> = {
    local: 'Local (TEI)',
    jina: 'Jina AI',
    openai: 'OpenAI',
    cohere: 'Cohere',
    bedrock: 'AWS Bedrock',
}

export const EMAIL_PROVIDER_TYPES = ['acs', 'resend', 'smtp'] as const
export type EmailProviderType = (typeof EMAIL_PROVIDER_TYPES)[number]

export const EMAIL_PROVIDER_LABELS: Record<EmailProviderType, string> = {
    acs: 'Azure Communication Services',
    resend: 'Resend',
    smtp: 'SMTP',
}

export const WEB_SEARCH_PROVIDER_TYPES = ['exa', 'serper', 'brave'] as const
export type WebSearchProviderType = (typeof WEB_SEARCH_PROVIDER_TYPES)[number]

export const WEB_SEARCH_PROVIDER_LABELS: Record<WebSearchProviderType, string> = {
    exa: 'Exa',
    serper: 'Serper',
    brave: 'Brave Search',
}

export const WEB_FETCH_PROVIDER_TYPES = ['exa', 'firecrawl'] as const
export type WebFetchProviderType = (typeof WEB_FETCH_PROVIDER_TYPES)[number]

export const WEB_FETCH_PROVIDER_LABELS: Record<WebFetchProviderType, string> = {
    exa: 'Exa',
    firecrawl: 'Firecrawl',
}
