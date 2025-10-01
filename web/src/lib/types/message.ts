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
    role: 'user' | 'assistant'
    content: MessageContent
}
