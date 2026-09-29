import type { StreamStatus } from '$lib/types/stream-status'

export function shouldShowEmptyStreamError(input: {
    messageEventsReceived: number
    pauseEventReceived: boolean
    hasError: boolean
    wasStopping: boolean
    iterationLimitReached: boolean
}): boolean {
    return (
        input.messageEventsReceived === 0 &&
        !input.pauseEventReceived &&
        !input.hasError &&
        !input.wasStopping &&
        !input.iterationLimitReached
    )
}

export function isIterationLimitTerminal(data: string): boolean {
    try {
        const payload: unknown = JSON.parse(data)
        return (
            typeof payload === 'object' &&
            payload !== null &&
            'reason' in payload &&
            payload.reason === 'iteration_limit'
        )
    } catch {
        return false
    }
}

export async function fetchChatStreamStatus(chatId: string): Promise<StreamStatus | null> {
    const response = await fetch(`/api/chat/${chatId}/stream/status`)
    if (!response.ok) return null
    return (await response.json()) as StreamStatus
}
