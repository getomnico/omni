export const GOOGLE_DRIVE_FOLDER_SEARCH_MIN_CHARS = 2
export const GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS = 250

export type GoogleDriveFolderSearchTimer = ReturnType<typeof setTimeout>

export function googleDriveFolderSearchLength(query: string): number {
    return Array.from(query.trim()).length
}

export function scheduleGoogleDriveFolderSearch(
    pendingTimer: GoogleDriveFolderSearchTimer | undefined,
    query: string,
    search: (normalizedQuery: string) => void,
): GoogleDriveFolderSearchTimer | undefined {
    if (pendingTimer) clearTimeout(pendingTimer)

    const normalizedQuery = query.trim()
    if (googleDriveFolderSearchLength(normalizedQuery) < GOOGLE_DRIVE_FOLDER_SEARCH_MIN_CHARS) {
        return undefined
    }

    return setTimeout(() => search(normalizedQuery), GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS)
}
