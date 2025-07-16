import { oauth } from '../config'
import { OAuthStateManager } from './state'
import type { OAuthConfig, OAuthProfile, OAuthTokens, OAuthError } from './types'

export class GoogleOAuthService {
    private static readonly GOOGLE_CONFIG: OAuthConfig = {
        clientId: oauth.google.clientId,
        clientSecret: oauth.google.clientSecret,
        redirectUri: oauth.google.redirectUri,
        scopes: oauth.google.scopes,
        authorizationEndpoint: 'https://accounts.google.com/o/oauth2/v2/auth',
        tokenEndpoint: 'https://oauth2.googleapis.com/token',
        userinfoEndpoint: 'https://openidconnect.googleapis.com/v1/userinfo',
    }

    static isConfigured(): boolean {
        return !!(
            this.GOOGLE_CONFIG.clientId &&
            this.GOOGLE_CONFIG.clientSecret &&
            this.GOOGLE_CONFIG.redirectUri
        )
    }

    static async generateAuthUrl(redirectUri?: string, userId?: string): Promise<string> {
        if (!this.isConfigured()) {
            throw new Error('Google OAuth is not configured')
        }

        const { stateToken, nonce } = await OAuthStateManager.createState(
            'google',
            redirectUri,
            userId,
            { nonce },
        )

        const params = new URLSearchParams({
            client_id: this.GOOGLE_CONFIG.clientId,
            redirect_uri: this.GOOGLE_CONFIG.redirectUri,
            response_type: 'code',
            scope: this.GOOGLE_CONFIG.scopes.join(' '),
            state: stateToken,
            access_type: 'offline',
            prompt: 'consent',
            nonce: nonce,
            hd: '*', // Allow any Google Workspace domain
        })

        return `${this.GOOGLE_CONFIG.authorizationEndpoint}?${params.toString()}`
    }

    static async exchangeCodeForTokens(
        code: string,
        stateToken: string,
    ): Promise<{
        tokens: OAuthTokens
        state: any
    }> {
        const state = await OAuthStateManager.validateAndConsumeState(stateToken)
        if (!state) {
            throw new Error('Invalid or expired OAuth state')
        }

        const tokenParams = new URLSearchParams({
            client_id: this.GOOGLE_CONFIG.clientId,
            client_secret: this.GOOGLE_CONFIG.clientSecret,
            code: code,
            grant_type: 'authorization_code',
            redirect_uri: this.GOOGLE_CONFIG.redirectUri,
        })

        const response = await fetch(this.GOOGLE_CONFIG.tokenEndpoint, {
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
        const response = await fetch(this.GOOGLE_CONFIG.userinfoEndpoint, {
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
        const params = new URLSearchParams({
            client_id: this.GOOGLE_CONFIG.clientId,
            client_secret: this.GOOGLE_CONFIG.clientSecret,
            refresh_token: refreshToken,
            grant_type: 'refresh_token',
        })

        const response = await fetch(this.GOOGLE_CONFIG.tokenEndpoint, {
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
