import { SourceType } from '$lib/types'

// Import icons as modules for proper Vite handling
import googleDriveIcon from '$lib/images/icons/google-drive.svg'
import googleDocsIcon from '$lib/images/icons/google-docs.svg'
import googleSheetsIcon from '$lib/images/icons/google-sheets.svg'
import googleSlidesIcon from '$lib/images/icons/google-slides.svg'
import gmailIcon from '$lib/images/icons/gmail.svg'
import slackIcon from '$lib/images/icons/slack.svg'
import atlassianIcon from '$lib/images/icons/atlassian.svg'
import confluenceIcon from '$lib/images/icons/confluence.svg'
import jiraIcon from '$lib/images/icons/jira.svg'
import firefliesIcon from '$lib/images/icons/fireflies.svg'
import hubspotIcon from '$lib/images/icons/hubspot.svg'
import microsoftIcon from '$lib/images/icons/microsoft.svg'
import oneDriveIcon from '$lib/images/icons/onedrive.svg'
import outlookIcon from '$lib/images/icons/outlook.svg'
import sharePointIcon from '$lib/images/icons/sharepoint.svg'
import clickupIcon from '$lib/images/icons/clickup.svg'
import notionIcon from '$lib/images/icons/notion.svg'

// Google Workspace MIME types
const GOOGLE_DOCS_MIMETYPES = [
    'application/vnd.google-apps.document',
    'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    'application/msword',
    'text/plain',
    'text/rtf',
]

const GOOGLE_SHEETS_MIMETYPES = [
    'application/vnd.google-apps.spreadsheet',
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    'application/vnd.ms-excel',
    'text/csv',
]

const GOOGLE_SLIDES_MIMETYPES = [
    'application/vnd.google-apps.presentation',
    'application/vnd.openxmlformats-officedocument.presentationml.presentation',
    'application/vnd.ms-powerpoint',
]

// Get icon based on source type and content type
export function getDocumentIconPath(sourceType: string, contentType: string): string | null {
    // For Gmail, always use Gmail icon
    if (sourceType === SourceType.GMAIL) {
        return gmailIcon
    }

    // For Google Drive, check content type to determine specific icon
    if (sourceType === SourceType.GOOGLE_DRIVE) {
        if (contentType === 'document' || GOOGLE_DOCS_MIMETYPES.includes(contentType)) {
            return googleDocsIcon
        }
        if (contentType === 'spreadsheet' || GOOGLE_SHEETS_MIMETYPES.includes(contentType)) {
            return googleSheetsIcon
        }
        if (contentType === 'presentation' || GOOGLE_SLIDES_MIMETYPES.includes(contentType)) {
            return googleSlidesIcon
        }
        // Default to generic Google Drive icon for other file types
        return googleDriveIcon
    } else if (sourceType === SourceType.CONFLUENCE) {
        return confluenceIcon
    } else if (sourceType === SourceType.JIRA) {
        return jiraIcon
    } else if (sourceType === SourceType.SLACK) {
        return slackIcon
    } else if (sourceType === SourceType.FIREFLIES) {
        return firefliesIcon
    } else if (sourceType === SourceType.HUBSPOT) {
        return hubspotIcon
    } else if (sourceType === SourceType.NOTION) {
        return notionIcon
    }

    // For other source types, return null (will use fallback icon)
    return null
}

// Map source types to icon file paths (legacy function, kept for backward compatibility)
export function getSourceIconPath(sourceType: string): string | null {
    switch (sourceType) {
        case SourceType.GOOGLE_DRIVE:
            return googleDriveIcon
        case SourceType.GMAIL:
            return gmailIcon
        case SourceType.SLACK:
            return slackIcon
        case SourceType.CONFLUENCE:
            return confluenceIcon
        case SourceType.JIRA:
            return jiraIcon
        case SourceType.FIREFLIES:
            return firefliesIcon
        case SourceType.HUBSPOT:
            return hubspotIcon
        case SourceType.ONE_DRIVE:
            return oneDriveIcon
        case SourceType.OUTLOOK:
        case SourceType.OUTLOOK_CALENDAR:
            return outlookIcon
        case SourceType.SHARE_POINT:
            return sharePointIcon
        case SourceType.GITHUB:
            return null // TODO: Add github icon when available
        case SourceType.LOCAL_FILES:
            return null // Use fallback FileText icon
        case SourceType.IMAP:
            return null // Uses Mail lucide icon
        case SourceType.CLICKUP:
            return clickupIcon
        case SourceType.NOTION:
            return notionIcon
        default:
            return null // Use fallback FileText icon
    }
}

// Get source type from source ID using sources lookup
export function getSourceTypeFromId(sourceId: string, sources: any[]): string | null {
    if (!sources) return null
    const source = sources.find((s) => s.id === sourceId)
    return source?.sourceType || null
}

// Parse metadata from URL hash fragment
// Expected format: url#meta=source_type,content_type
export function parseUrlMetadata(url: string): { sourceType?: string; contentType?: string } {
    try {
        const hashIndex = url.indexOf('#meta=')
        if (hashIndex === -1) return {}

        const metaString = url.substring(hashIndex + 6) // Skip '#meta='
        const parts = metaString.split(',')

        if (parts.length === 0) return {}
        if (parts.length === 1) {
            // Could be either source_type or content_type
            // If it contains '/', it's likely a content_type
            if (parts[0].includes('/')) {
                return { contentType: parts[0] }
            } else {
                return { sourceType: parts[0] }
            }
        }

        return {
            sourceType: parts[0],
            contentType: parts[1],
        }
    } catch {
        return {}
    }
}

// Infer source type from URL patterns (fallback)
export function inferSourceFromUrl(url: string): SourceType | null {
    if (!url) return null

    const urlLower = url.toLowerCase()

    if (urlLower.includes('docs.google.com')) return SourceType.GOOGLE_DRIVE
    if (urlLower.includes('drive.google.com')) return SourceType.GOOGLE_DRIVE
    if (urlLower.includes('sheets.google.com')) return SourceType.GOOGLE_DRIVE
    if (urlLower.includes('slides.google.com')) return SourceType.GOOGLE_DRIVE
    if (urlLower.includes('mail.google.com') || urlLower.includes('gmail.com'))
        return SourceType.GMAIL
    if (urlLower.includes('slack.com')) return SourceType.SLACK
    if (urlLower.includes('atlassian.net/spaces')) return SourceType.CONFLUENCE
    if (urlLower.includes('atlassian.net/jira')) return SourceType.JIRA
    if (urlLower.includes('github.com')) return SourceType.GITHUB
    if (urlLower.includes('fireflies.ai')) return SourceType.FIREFLIES

    return null
}

// Get icon from search result URL (main function for tool-message component)
export function getIconFromSearchResult(sourceUrl: string): string | null {
    if (!sourceUrl) return null

    // First, try to parse metadata from URL hash
    const metadata = parseUrlMetadata(sourceUrl)

    // Try to get icon from content_type if available
    if (metadata.contentType) {
        const sourceType = metadata.sourceType || inferSourceFromUrl(sourceUrl)
        if (sourceType) {
            const icon = getDocumentIconPath(sourceType, metadata.contentType)
            if (icon) return icon
        }
    }

    // Try to get icon from source_type in metadata
    if (metadata.sourceType) {
        const icon = getSourceIconPath(metadata.sourceType)
        if (icon) return icon
    }

    // Fallback: infer from URL patterns
    const inferredSourceType = inferSourceFromUrl(sourceUrl)
    if (inferredSourceType) {
        return getSourceIconPath(inferredSourceType)
    }

    return null
}

export function getSourceDisplayName(sourceType: SourceType) {
    const sourceDisplayNames: Record<string, string> = {
        [SourceType.GOOGLE_DRIVE]: 'Google Drive',
        [SourceType.GMAIL]: 'Gmail',
        [SourceType.CONFLUENCE]: 'Confluence',
        [SourceType.JIRA]: 'Jira',
        [SourceType.SLACK]: 'Slack',
        [SourceType.GITHUB]: 'GitHub',
        [SourceType.LOCAL_FILES]: 'Files',
        [SourceType.WEB]: 'Web',
        [SourceType.FIREFLIES]: 'Fireflies',
        [SourceType.CLICKUP]: 'ClickUp',
        [SourceType.NOTION]: 'Notion',
    }

    return sourceDisplayNames[sourceType]
}

export function getSourceTypeFromDisplayName(displayName: string): SourceType | null {
    const lower = displayName.toLowerCase()
    for (const sourceType of Object.values(SourceType)) {
        if (getSourceDisplayName(sourceType)?.toLowerCase() === lower) {
            return sourceType
        }
    }
    return null
}
