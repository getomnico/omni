import { SourceType } from '$lib/types'

// Import icons as modules for proper Vite handling
import googleDriveIcon from '$lib/images/icons/google-drive.svg'
import googleDocsIcon from '$lib/images/icons/google-docs.svg'
import googleSheetsIcon from '$lib/images/icons/google-sheets.svg'
import googleSlidesIcon from '$lib/images/icons/google-slides.svg'
import gmailIcon from '$lib/images/icons/gmail.svg'

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
    console.log(`Get icon for source ${sourceType}, content type ${contentType}`)
    // For Gmail, always use Gmail icon
    if (sourceType === SourceType.GMAIL) {
        return gmailIcon
    }

    // For Google Drive, check content type to determine specific icon
    if (sourceType === SourceType.GOOGLE_DRIVE) {
        if (GOOGLE_DOCS_MIMETYPES.includes(contentType)) {
            return googleDocsIcon
        }
        if (GOOGLE_SHEETS_MIMETYPES.includes(contentType)) {
            return googleSheetsIcon
        }
        if (GOOGLE_SLIDES_MIMETYPES.includes(contentType)) {
            return googleSlidesIcon
        }
        // Default to generic Google Drive icon for other file types
        return googleDriveIcon
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
            return null // TODO: Add slack icon when available
        case SourceType.CONFLUENCE:
            return null // TODO: Add confluence icon when available
        case SourceType.GITHUB:
            return null // TODO: Add github icon when available
        case SourceType.JIRA:
            return null // TODO: Add jira icon when available
        case SourceType.LOCAL_FILES:
            return null // Use fallback FileText icon
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
