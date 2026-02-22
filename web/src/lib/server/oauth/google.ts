import { getGoogleAuthConfig } from '../db/auth-providers'
import { app } from '../config'
import { OAuthStateManager } from './state'
import type { OAuthProfile, OAuthTokens, OAuthError } from './types'

const GOOGLE_AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const GOOGLE_TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const GOOGLE_USERINFO_ENDPOINT = 'https://openidconnect.googleapis.com/v1/userinfo'
const DEFAULT_SCOPES = ['openid', 'profile', 'email']

export class GoogleOAuthService {
    private static async loadConfig() {
        const config = await getGoogleAuthConfig()
        if (!config || !config.enabled || !config.clientId || !config.clientSecret) {
            return null
        }
        return {
            clientId: config.clientId,
            clientSecret: config.clientSecret,
            redirectUri: `${app.publicUrl}/auth/google/callback`,
            scopes: DEFAULT_SCOPES,
        }
    }

    static async isConfigured(): Promise<boolean> {
        const config = await this.loadConfig()
        return config !== null
    }

    static async generateAuthUrl(redirectUri?: string, userId?: string): Promise<string> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth is not configured')
        }

        const { stateToken, nonce } = await OAuthStateManager.createState(
            'google',
            redirectUri,
            userId,
        )

        const params = new URLSearchParams({
            client_id: config.clientId,
            redirect_uri: config.redirectUri,
            response_type: 'code',
            scope: config.scopes.join(' '),
            state: stateToken,
            access_type: 'offline',
            prompt: 'consent',
            nonce: nonce,
            hd: '*', // Allow any Google Workspace domain
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
            throw new Error('Google OAuth is not configured')
        }

        const state = await OAuthStateManager.validateAndConsumeState(stateToken)
        if (!state) {
            throw new Error('Invalid or expired OAuth state')
        }

        const tokenParams = new URLSearchParams({
            client_id: config.clientId,
            client_secret: config.clientSecret,
            code: code,
            grant_type: 'authorization_code',
            redirect_uri: config.redirectUri,
        })

        const response = await fetch(GOOGLE_TOKEN_ENDPOINT, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
            },
            body: tokenParams.toString(),
        })

        const data = await response.json()

        if (!response.ok) {
            const error = data as OAuthError
            throw new Error(
                `OAuth token exchange failed: ${error.error} - ${error.error_description}`,
            )
        }

        const tokens = data as OAuthTokens
        return { tokens, state }
    }

    static async fetchUserProfile(accessToken: string): Promise<OAuthProfile> {
        const response = await fetch(GOOGLE_USERINFO_ENDPOINT, {
            headers: {
                Authorization: `Bearer ${accessToken}`,
            },
        })

        if (!response.ok) {
            throw new Error(
                `Failed to fetch user profile: ${response.status} ${response.statusText}`,
            )
        }

        const profile = await response.json()

        return {
            id: profile.sub,
            email: profile.email,
            name: profile.name,
            given_name: profile.given_name,
            family_name: profile.family_name,
            picture: profile.picture,
            email_verified: profile.email_verified,
            locale: profile.locale,
            hd: profile.hd, // Google Workspace domain
        }
    }

    static async refreshToken(refreshToken: string): Promise<OAuthTokens> {
        const config = await this.loadConfig()
        if (!config) {
            throw new Error('Google OAuth is not configured')
        }

        const params = new URLSearchParams({
            client_id: config.clientId,
            client_secret: config.clientSecret,
            refresh_token: refreshToken,
            grant_type: 'refresh_token',
        })

        const response = await fetch(GOOGLE_TOKEN_ENDPOINT, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
            },
            body: params.toString(),
        })

        const data = await response.json()

        if (!response.ok) {
            const error = data as OAuthError
            throw new Error(`Token refresh failed: ${error.error} - ${error.error_description}`)
        }

        return data as OAuthTokens
    }
}
