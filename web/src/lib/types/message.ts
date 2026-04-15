import type { TextCitationParam } from '@anthropic-ai/sdk/resources/messages'

// Custom source variant embedded in Anthropic document/image blocks. Resolved to real
// content blocks by the AI service before sending to the LLM.
export type OmniUploadBlock = {
    type: 'document' | 'image'
    source: { type: 'omni_upload'; upload_id: string }
}

export type TextMessageContent = {
    id: number
    type: 'text'
    text: string
    citations?: Array<TextCitationParam>
}

export enum ToolApprovalStatus {
    Pending = 'pending',
    Approved = 'approved',
    Denied = 'denied',
}

export type ToolApproval = {
    status: ToolApprovalStatus
    approvalId: string
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
    approval?: ToolApproval
}

export type ApprovalRequiredEvent = {
    approval_id: string
    tool_name: string
    tool_input: Record<string, unknown>
    tool_call_id: string
}

export type ToolName = 'search_documents' | 'read_document' | string

export type UploadMessageContent = {
    id: number
    type: 'upload'
    uploadId: string
}

export type MessageContent = Array<TextMessageContent | ToolMessageContent | UploadMessageContent>
export type ProcessedMessage = {
    id: number
    // ID of the message in the db.
    // Multiple messages might be combined into a single ProcessedMessage, in that case, this field will contain the ID of the last message.
    origMessageId: string
    role: 'user' | 'assistant'
    content: MessageContent
    parentMessageId?: string
    siblingIds?: string[]
    siblingIndex?: number
    createdAt?: Date
}
