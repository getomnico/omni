import { describe, expect, it } from 'vitest'
import { isIterationLimitTerminal, shouldShowEmptyStreamError } from './stream-status'

describe('shouldShowEmptyStreamError', () => {
    const empty = {
        messageEventsReceived: 0,
        pauseEventReceived: false,
        hasError: false,
        wasStopping: false,
        iterationLimitReached: false,
    }

    it('suppresses the generic error for iteration-limited empty output', () => {
        expect(shouldShowEmptyStreamError({ ...empty, iterationLimitReached: true })).toBe(false)
    })

    it('keeps the generic error for unexpected empty output', () => {
        expect(shouldShowEmptyStreamError(empty)).toBe(true)
    })
})

describe('isIterationLimitTerminal', () => {
    it('recognizes the typed iteration-limit terminal reason', () => {
        expect(isIterationLimitTerminal('{"reason":"iteration_limit"}')).toBe(true)
    })

    it('does not treat other or malformed terminal events as limits', () => {
        expect(isIterationLimitTerminal('{"reason":"completed"}')).toBe(false)
        expect(isIterationLimitTerminal('{')).toBe(false)
    })
})
