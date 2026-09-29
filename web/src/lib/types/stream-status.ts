export type StreamStatus = {
    running: boolean
    resumable: boolean
    pendingApproval: boolean
    pendingOAuth: boolean
    pendingSteering: boolean
    iterationLimitReached: boolean
    iterationLimitMessageId: string | null
}
