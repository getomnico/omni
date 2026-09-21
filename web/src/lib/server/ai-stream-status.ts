import { env } from '$env/dynamic/private'
import type { StreamStatus } from '$lib/types/stream-status'

export async function getChatStreamStatus(chatId: string): Promise<StreamStatus> {
    const response = await fetch(`${env.AI_SERVICE_URL}/chat/${chatId}/stream/status`)
    if (!response.ok) {
        throw new Error(`AI stream status failed with status ${response.status}`)
    }

    const status = (await response.json()) as {
        running?: boolean
        resumable?: boolean
        pending_approval?: boolean
        pending_oauth?: boolean
        pending_steering?: boolean
    }
    const running = status.running === true
    const resumable = status.resumable === true
    const pendingApproval = status.pending_approval === true
    const pendingOAuth = status.pending_oauth === true
    const pendingSteering = status.pending_steering === true

    return {
        running,
        resumable,
        pendingApproval,
        pendingOAuth,
        pendingSteering,
    }
}
