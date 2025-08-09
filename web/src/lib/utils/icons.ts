import { SourceType } from '$lib/types'

// Import icons as modules for proper Vite handling
import googleDriveIcon from '$lib/images/icons/google-drive.svg'
import gmailIcon from '$lib/images/icons/gmail.svg'

// Map source types to icon file paths
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
