import { SourceType } from '$lib/types'

export function formatDate(date: Date | null) {
    if (!date) return 'Never'
    return new Date(date).toLocaleString()
}

const sourceNouns: Record<string, string> = {
    [SourceType.GOOGLE_DRIVE]: 'documents',
    [SourceType.GMAIL]: 'threads',
    [SourceType.SLACK]: 'messages',
    [SourceType.CONFLUENCE]: 'pages',
    [SourceType.JIRA]: 'issues',
    [SourceType.HUBSPOT]: 'records',
    [SourceType.FIREFLIES]: 'transcripts',
    [SourceType.IMAP]: 'emails',
    [SourceType.ONE_DRIVE]: 'files',
    [SourceType.OUTLOOK]: 'emails',
    [SourceType.OUTLOOK_CALENDAR]: 'events',
    [SourceType.SHARE_POINT]: 'documents',
    [SourceType.WEB]: 'pages',
    [SourceType.LINEAR]: 'items',
    [SourceType.LOCAL_FILES]: 'files',
    [SourceType.CLICKUP]: 'tasks',
    [SourceType.NOTION]: 'pages',
    [SourceType.GITHUB]: 'documents',
    [SourceType.PAPERLESS_NGX]: 'documents',
    [SourceType.NEXTCLOUD]: 'files',
}

export function getSourceNoun(sourceType: SourceType): string {
    return sourceNouns[sourceType] ?? 'documents'
}

export function getStatusColor(isActive: boolean) {
    return isActive
        ? 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
        : 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-300'
}
