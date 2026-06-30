import { env } from '$env/dynamic/private'

export type StreamStatus = {
    running: boolean
    resumable: boolean
    pendingApproval: boolean
    pendingOAuth: boolean
}

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
    }
    const running = status.running === true
    const resumable = status.resumable === true
    const pendingApproval = status.pending_approval === true
    const pendingOAuth = status.pending_oauth === true

    return {
        running,
        resumable,
        pendingApproval,
        pendingOAuth,
    }
}
