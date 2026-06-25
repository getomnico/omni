// Wire types for the web server -> AI service provider test endpoint,
// plus chat stream error payloads.

export type ProviderType =
    | 'anthropic'
    | 'openai'
    | 'gemini'
    | 'bedrock'
    | 'vertex_ai'
    | 'azure_foundry'
    | 'openai_compatible'

export interface TestModelRequest {
    api_key?: string | null
    model?: string | null
    region_name?: string | null
    model_id?: string | null
    region?: string | null
    project_id?: string | null
    endpoint_url?: string | null
    base_url?: string | null
}

export interface TestModelResponse {
    ok: boolean
    error: string | null
    provider: ProviderType | null
    status_code: number | null
    model: string | null
    latency_ms: number | null
}

export interface AvailableModel {
    model_id: string
    display_name: string
}

export interface ListProviderModelsResponse {
    models: AvailableModel[]
}

// SSE `event: stream_error` payload from /chat/{id}/stream.
export interface StreamErrorEvent {
    message: string
    provider?: ProviderType | null
    model?: string | null
    statusCode?: number | null
}
