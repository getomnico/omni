import { env } from '$env/dynamic/private'

type AIStreamStatus = {
    running?: boolean
    resumable?: boolean
    pending_approval?: boolean
    pending_oauth?: boolean
}

export type ChatStreamStatus = {
    running: boolean
    resumable: boolean
    pendingApproval: boolean
    pendingOAuth: boolean
    active: boolean
}

export async function getChatStreamStatus(chatId: string): Promise<ChatStreamStatus> {
    const response = await fetch(`${env.AI_SERVICE_URL}/chat/${chatId}/stream_status`)
    if (!response.ok) {
        throw new Error(`AI stream status failed with status ${response.status}`)
    }

    const status = (await response.json()) as AIStreamStatus
    const running = status.running === true
    const resumable = status.resumable === true
    const pendingApproval = status.pending_approval === true
    const pendingOAuth = status.pending_oauth === true

    return {
        running,
        resumable,
        pendingApproval,
        pendingOAuth,
        active: running || resumable || pendingApproval || pendingOAuth,
    }
}
