export interface OAuthState {
    id: string
    state_token: string
    provider: string
    redirect_uri?: string
    nonce?: string
    code_verifier?: string
    user_id?: string
    expires_at: Date
    created_at: Date
    metadata: Record<string, any>
}

export interface OAuthProfile {
    id: string // Provider-specific user ID
    email: string
    name?: string
    given_name?: string
    family_name?: string
    picture?: string
    email_verified?: boolean
    locale?: string
    hd?: string // Google Workspace domain
}

export interface OAuthTokens {
    access_token: string
    refresh_token?: string
    token_type: string
    expires_in?: number
    scope?: string
    id_token?: string
}

export interface OAuthError {
    error: string
    error_description?: string
    error_uri?: string
}

export interface OAuthConfig {
    clientId: string
    clientSecret: string
    redirectUri: string
    scopes: string[]
    authorizationEndpoint: string
    tokenEndpoint: string
    userinfoEndpoint: string
}
