import type { Chat } from '$lib/server/db/schema'

export type SerializedChat = Omit<Chat, 'createdAt' | 'updatedAt'> & {
    createdAt: string
    updatedAt: string
}

export type HighlightPart = { text: string; match: boolean }

export type ChatSearchSnippet = {
    source: 'title' | 'message'
    messageId: string | null
    parts: HighlightPart[]
}

export type ChatSearchResult = Chat & {
    snippet: ChatSearchSnippet | null
}

export type SerializedChatSearchResult = SerializedChat & {
    snippet: ChatSearchSnippet | null
}
