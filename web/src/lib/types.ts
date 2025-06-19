export enum SourceType {
    GOOGLE_DRIVE = 'google_drive',
    GMAIL = 'gmail',
    CONFLUENCE = 'confluence',
    JIRA = 'jira',
    SLACK = 'slack',
    GITHUB = 'github',
    LOCAL_FILES = 'local_files',
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
