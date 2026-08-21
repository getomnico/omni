import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
    GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS,
    scheduleGoogleDriveFolderSearch,
    type GoogleDriveFolderSearchTimer,
} from './google-drive-folder-search'

describe('scheduleGoogleDriveFolderSearch', () => {
    beforeEach(() => {
        vi.useFakeTimers()
    })

    afterEach(() => {
        vi.useRealTimers()
    })

    it('does not schedule empty or one-character searches', () => {
        const search = vi.fn()

        expect(scheduleGoogleDriveFolderSearch(undefined, '', search)).toBeUndefined()
        expect(scheduleGoogleDriveFolderSearch(undefined, ' a ', search)).toBeUndefined()
        expect(scheduleGoogleDriveFolderSearch(undefined, '文', search)).toBeUndefined()

        vi.runAllTimers()
        expect(search).not.toHaveBeenCalled()
    })

    it('waits for the debounce period before searching a trimmed query', () => {
        const search = vi.fn()

        const timer = scheduleGoogleDriveFolderSearch(undefined, '  ab  ', search)

        expect(timer).toBeDefined()
        vi.advanceTimersByTime(GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS - 1)
        expect(search).not.toHaveBeenCalled()

        vi.advanceTimersByTime(1)
        expect(search).toHaveBeenCalledOnce()
        expect(search).toHaveBeenCalledWith('ab')
    })

    it('cancels the pending search when the user keeps typing', () => {
        const search = vi.fn()
        let timer: GoogleDriveFolderSearchTimer | undefined

        timer = scheduleGoogleDriveFolderSearch(timer, 'ro', search)
        vi.advanceTimersByTime(GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS - 1)
        timer = scheduleGoogleDriveFolderSearch(timer, 'roadmap', search)
        vi.advanceTimersByTime(GOOGLE_DRIVE_FOLDER_SEARCH_DEBOUNCE_MS)

        expect(search).toHaveBeenCalledOnce()
        expect(search).toHaveBeenCalledWith('roadmap')
    })

    it('cancels a pending search when the input becomes too short', () => {
        const search = vi.fn()
        let timer = scheduleGoogleDriveFolderSearch(undefined, 'roadmap', search)

        timer = scheduleGoogleDriveFolderSearch(timer, 'r', search)
        expect(timer).toBeUndefined()

        vi.runAllTimers()
        expect(search).not.toHaveBeenCalled()
    })
})
