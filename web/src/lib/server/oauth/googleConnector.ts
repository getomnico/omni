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
