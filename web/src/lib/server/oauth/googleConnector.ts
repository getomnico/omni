import { getConnectorConfig } from '../db/connector-configs'
import { app } from '../config'
import { OAuthStateManager } from './state'
import type { OAuthTokens, OAuthError } from './types'

const GOOGLE_AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const GOOGLE_TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const GOOGLE_USERINFO_ENDPOINT = 'https://www.googleapis.com/oauth2/v3/userinfo'

function getScopesForSourceType(sourceType: string): string[] {
    switch (sourceType) {
        case 'google_drive':
            return ['https://www.googleapis.com/auth/drive.readonly']
        case 'gmail':
            return ['https://www.googleapis.com/auth/gmail.readonly']
        default:
            return [
                'https://www.googleapis.com/auth/drive.readonly',
                'https://www.googleapis.com/auth/gmail.readonly',
            ]
    }
}

/// Scopes required for per-user write actions (MCP write tools) against a
/// connected Google source. Used by the `/api/sources/[sourceId]/user-auth/...`
/// flow that attaches per-user credentials to org-wide sources.
export function getWriteScopesForSourceType(sourceType: string): string[] {
    switch (sourceType) {
        case 'google_drive':
            // drive.file scopes the grant to files the app creates/opens — the safe
            // default for MCP write tools. Switch to 'drive' if a connector needs
            // full-mailbox-style access.
            return ['https://www.googleapis.com/auth/drive.file']
        case 'gmail':
            // gmail.send covers send-as; gmail.modify is needed for label/thread
            // mutations. Start with both and trim when a write tool needs less.
            return [
                'https://www.googleapis.com/auth/gmail.send',
                'https://www.googleapis.com/auth/gmail.modify',
            ]
        default:
            return []
    }
}

export class GoogleConnectorOAuthService {
    private static async loadConfig() {
        const row = await getConnectorConfig('google')
        if (!row) return null

        const config = row.config as Record<string, string>
        const clientId = config.oauth_client_id
        const clientSecret = config.oauth_client_secret

        if (!clientId || !clientSecret) return null

        return {
            clientId,
            clientSecret,
            redirectUri: `${app.publicUrl}/api/connectors/google/oauth/callback`,
        }
    }

    static async isConfigured(): Promise<boolean> {
        const config = await this.loadConfig()
        return config !== null
    }

    static async generateAuthUrl(serviceTypes: string[], userId: string): Promise<string> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth connector is not configured')
        }

        const { stateToken } = await OAuthStateManager.createState(
            'google_connector',
            undefined,
            userId,
            { serviceTypes },
        )

        const connectorScopes = [...new Set(serviceTypes.flatMap((t) => getScopesForSourceType(t)))]
        const scopes = ['email', 'profile', ...connectorScopes]

        const params = new URLSearchParams({
            client_id: config.clientId,
            redirect_uri: config.redirectUri,
            response_type: 'code',
            scope: scopes.join(' '),
            state: stateToken,
            access_type: 'offline',
            prompt: 'consent',
        })

        return `${GOOGLE_AUTH_ENDPOINT}?${params.toString()}`
    }

    /// Build the OAuth URL for attaching a user's write credentials to an
    /// existing org-wide Google source. State carries the target `sourceId` so
    /// the callback knows which source the resulting credential belongs to.
    static async generateUserWriteAuthUrl(
        sourceId: string,
        sourceType: string,
        userId: string,
        returnTo?: string,
    ): Promise<{ url: string; requiredScopes: string[] }> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth connector is not configured')
        }

        const requiredScopes = getWriteScopesForSourceType(sourceType)
        if (requiredScopes.length === 0) {
            throw new Error(`No write scopes defined for source_type=${sourceType}`)
        }

        const { stateToken } = await OAuthStateManager.createState(
            'google_user_write',
            undefined,
            userId,
            { sourceId, sourceType, requiredScopes, returnTo },
        )

        // 'email' lets the callback identify which Google account the user granted
        // (stored in service_credentials.principal_email for display).
        const scopes = ['email', ...requiredScopes]

        const params = new URLSearchParams({
            client_id: config.clientId,
            redirect_uri: `${app.publicUrl}/api/sources/${sourceId}/user-auth/callback`,
            response_type: 'code',
            scope: scopes.join(' '),
            state: stateToken,
            access_type: 'offline',
            prompt: 'consent',
        })

        return {
            url: `${GOOGLE_AUTH_ENDPOINT}?${params.toString()}`,
            requiredScopes,
        }
    }

    /// Variant of exchangeCodeForTokens for the per-user write flow — uses the
    /// same client config but with a source-specific redirect URI.
    static async exchangeUserWriteCode(
        sourceId: string,
        code: string,
        stateToken: string,
    ): Promise<{ tokens: OAuthTokens; state: any }> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth connector is not configured')
        }

        const state = await OAuthStateManager.validateAndConsumeState(stateToken)
        if (!state || state.provider !== 'google_user_write') {
            throw new Error('Invalid or expired OAuth state')
        }
        if (!state.metadata || state.metadata.sourceId !== sourceId) {
            throw new Error('OAuth state does not match source')
        }

        const tokenParams = new URLSearchParams({
            client_id: config.clientId,
            client_secret: config.clientSecret,
            code,
            grant_type: 'authorization_code',
            redirect_uri: `${app.publicUrl}/api/sources/${sourceId}/user-auth/callback`,
        })

        const response = await fetch(GOOGLE_TOKEN_ENDPOINT, {
            method: 'POST',
            headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
            body: tokenParams.toString(),
        })

        const data = await response.json()

        if (!response.ok) {
            const err = data as OAuthError
            throw new Error(`OAuth token exchange failed: ${err.error} - ${err.error_description}`)
        }

        return { tokens: data as OAuthTokens, state }
    }

    static async exchangeCodeForTokens(
        code: string,
        stateToken: string,
    ): Promise<{
        tokens: OAuthTokens
        state: any
    }> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth connector is not configured')
        }

        const state = await OAuthStateManager.validateAndConsumeState(stateToken)
        if (!state) {
            throw new Error('Invalid or expired OAuth state')
        }

        const tokenParams = new URLSearchParams({
            client_id: config.clientId,
            client_secret: config.clientSecret,
            code,
            grant_type: 'authorization_code',
            redirect_uri: config.redirectUri,
        })

        const response = await fetch(GOOGLE_TOKEN_ENDPOINT, {
            method: 'POST',
            headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
            body: tokenParams.toString(),
        })

        const data = await response.json()

        if (!response.ok) {
            const error = data as OAuthError
            throw new Error(
                `OAuth token exchange failed: ${error.error} - ${error.error_description}`,
            )
        }

        return { tokens: data as OAuthTokens, state }
    }

    static async fetchUserEmail(accessToken: string): Promise<string> {
        const response = await fetch(GOOGLE_USERINFO_ENDPOINT, {
            headers: { Authorization: `Bearer ${accessToken}` },
        })

        if (!response.ok) {
            throw new Error(
                `Failed to fetch user profile: ${response.status} ${response.statusText}`,
            )
        }

        const profile = await response.json()
        return profile.email
    }
}
