import type { StreamStatus } from '$lib/types/stream-status'

export async function fetchChatStreamStatus(chatId: string): Promise<StreamStatus | null> {
    const response = await fetch(`/api/chat/${chatId}/stream/status`)
    if (!response.ok) return null
    return (await response.json()) as StreamStatus
}
