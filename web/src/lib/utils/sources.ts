import { SourceType } from '$lib/types'
import { formatDateTime } from '$lib/utils/datetime'

export function formatDate(date: Date | null, timeZone?: string | null) {
    if (!date) return 'Never'
    return formatDateTime(date, timeZone)
}

export function formatSyncRunDate(value: Date | string | null, timeZone?: string | null) {
    return value ? formatDateTime(value, timeZone) : '-'
}

export function formatSyncRunDuration(
    startedAt: Date | string | null,
    completedAt: Date | string | null,
) {
    if (!startedAt || !completedAt) return '-'
    const ms = new Date(completedAt).getTime() - new Date(startedAt).getTime()
    if (!Number.isFinite(ms) || ms < 0) return '-'
    const seconds = Math.round(ms / 1000)
    if (seconds < 60) return `${seconds}s`
    const minutes = Math.floor(seconds / 60)
    const remainingSeconds = seconds % 60
    return `${minutes}m ${remainingSeconds}s`
}

const sourceNouns: Record<string, string> = {
    [SourceType.GOOGLE_DRIVE]: 'documents',
    [SourceType.GMAIL]: 'threads',
    [SourceType.SLACK]: 'threads',
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

export function getSyncRunStatusColor(status: string) {
    switch (status.toLowerCase()) {
        case 'completed':
            return 'bg-green-100 text-green-800 dark:bg-green-900/20 dark:text-green-400'
        case 'failed':
            return 'bg-red-100 text-red-800 dark:bg-red-900/20 dark:text-red-400'
        case 'running':
            return 'bg-blue-100 text-blue-800 dark:bg-blue-900/20 dark:text-blue-400'
        case 'cancelled':
            return 'bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-300'
        default:
            return 'bg-muted text-muted-foreground'
    }
}
