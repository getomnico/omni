export type ApiKeyScope = 'public' | 'user'

export function parseApiKeyScope(value: unknown): ApiKeyScope | null {
    return value === 'public' || value === 'user' ? value : null
}
