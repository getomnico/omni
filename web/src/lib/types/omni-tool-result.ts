/// Typed envelopes for structured tool_result content blocks.
///
/// Mirrors `services/ai/tools/omni_tool_result.py`. When a tool surfaces a
/// UI-driven prompt (e.g. "user must complete OAuth") rather than a normal
/// action result, the AI service encodes a typed envelope inside the
/// tool_result's text content. The frontend parses it here and dispatches on
/// `omni_kind` to render the right card.

export const OmniToolResultKind = {
    OauthRequired: 'oauth_required',
    ApprovalRequired: 'approval_required',
} as const

export type OmniToolResultKind = (typeof OmniToolResultKind)[keyof typeof OmniToolResultKind]

export type OAuthRequiredPayload = {
    source_id: string
    source_type: string
    provider: string
    /** Relative URL on the omni-web host, e.g. `/api/oauth/start?source_id=...`. */
    oauth_start_url: string
}

export type ApprovalRequiredPayload = {
    approval_id: string
    tool_name: string
    tool_input: Record<string, unknown>
    tool_call_id: string
    source_id?: string | null
    source_type?: string | null
}

export type OmniToolResultEnvelope =
    | {
          omni_kind: typeof OmniToolResultKind.OauthRequired
          payload: OAuthRequiredPayload
      }
    | {
          omni_kind: typeof OmniToolResultKind.ApprovalRequired
          payload: ApprovalRequiredPayload
      }

/**
 * Best-effort parse of a tool_result text block as an Omni envelope. Returns
 * null if the text isn't a recognized envelope (in which case the caller
 * should render the text as-is).
 */
export function tryParseOmniEnvelope(text: string): OmniToolResultEnvelope | null {
    if (!text || text[0] !== '{') return null
    let obj: unknown
    try {
        obj = JSON.parse(text)
    } catch {
        return null
    }
    if (!obj || typeof obj !== 'object') return null
    const candidate = obj as Record<string, unknown>
    const kind = candidate.omni_kind
    const payload = candidate.payload
    if (typeof kind !== 'string' || !payload || typeof payload !== 'object') {
        return null
    }
    if (kind === OmniToolResultKind.OauthRequired) {
        const p = payload as Record<string, unknown>
        if (
            typeof p.source_id !== 'string' ||
            typeof p.source_type !== 'string' ||
            typeof p.provider !== 'string' ||
            typeof p.oauth_start_url !== 'string'
        ) {
            return null
        }
        return {
            omni_kind: OmniToolResultKind.OauthRequired,
            payload: {
                source_id: p.source_id,
                source_type: p.source_type,
                provider: p.provider,
                oauth_start_url: p.oauth_start_url,
            },
        }
    }
    if (kind === OmniToolResultKind.ApprovalRequired) {
        const p = payload as Record<string, unknown>
        if (
            typeof p.approval_id !== 'string' ||
            typeof p.tool_name !== 'string' ||
            typeof p.tool_call_id !== 'string' ||
            !p.tool_input ||
            typeof p.tool_input !== 'object'
        ) {
            return null
        }
        return {
            omni_kind: OmniToolResultKind.ApprovalRequired,
            payload: {
                approval_id: p.approval_id,
                tool_name: p.tool_name,
                tool_input: p.tool_input as Record<string, unknown>,
                tool_call_id: p.tool_call_id,
                source_id: typeof p.source_id === 'string' ? p.source_id : null,
                source_type: typeof p.source_type === 'string' ? p.source_type : null,
            },
        }
    }
    return null
}
