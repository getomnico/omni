import type { TextCitationParam } from '@anthropic-ai/sdk/resources/messages'

export type TextMessageContent = {
    id: number
    type: 'text'
    text: string
    citations?: Array<TextCitationParam>
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
    // For connector action tools
    actionResult?: {
        toolUseId: string
        text: string
        isError: boolean
    }
    // Approval state for write actions
    approval?: {
        status: 'pending' | 'approved' | 'denied'
        approvalId: string
    }
}

export type ApprovalRequiredEvent = {
    approval_id: string
    tool_name: string
    tool_input: Record<string, unknown>
    tool_call_id: string
}

export type ToolName = 'search_documents' | 'read_document' | string

export type MessageContent = Array<TextMessageContent | ToolMessageContent>
export type ProcessedMessage = {
    id: number
    // ID of the message in the db.
    // Multiple messages might be combined into a single ProcessedMessage, in that case, this field will contain the ID of the last message.
    origMessageId: string
    role: 'user' | 'assistant'
    content: MessageContent
}
