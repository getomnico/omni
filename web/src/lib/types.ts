export enum SourceType {
    GOOGLE_DRIVE = 'google_drive',
    GMAIL = 'gmail',
    CONFLUENCE = 'confluence',
    JIRA = 'jira',
    SLACK = 'slack',
    GITHUB = 'github',
    LOCAL_FILES = 'local_files',
    WEB = 'web',
}

export enum ServiceProvider {
    GOOGLE = 'google',
    SLACK = 'slack',
    ATLASSIAN = 'atlassian',
    GITHUB = 'github',
    MICROSOFT = 'microsoft',
}

export enum AuthType {
    JWT = 'jwt',
    API_KEY = 'api_key',
    BASIC_AUTH = 'basic_auth',
    BEARER_TOKEN = 'bearer_token',
    BOT_TOKEN = 'bot_token',
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
