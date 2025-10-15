export type TextMessageContent = {
    id: number
    type: 'text'
    text: string
}

export type ToolMessageContent = {
    id: number
    type: 'tool'
    toolUse: {
        id: string
        name: string
        input: any
    }
    toolResult?: {
        toolUseId: string // Same as toolUse.id
        content: {
            title: string
            source: string
        }[]
    }
}

export type MessageContent = Array<TextMessageContent | ToolMessageContent>
export type ProcessedMessage = {
    id: number
    // ID of the message in the db.
    // Multiple messages might be combined into a single ProcessedMessage, in that case, this field will contain the ID of the last message.
    origMessageId: string
    role: 'user' | 'assistant'
    content: MessageContent
}
