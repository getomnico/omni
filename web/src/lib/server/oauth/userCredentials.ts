import { db } from '../db'
import { sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import type { OAuthTokens, OAuthProfile } from './types'

// Raw database row type (snake_case from SQL)
type UserOAuthCredentialRow = {
    id: string
    user_id: string
    provider: string
    provider_user_id: string
    access_token: string | null
    refresh_token: string | null
    token_type: string
    expires_at: Date | null
    scopes: string[] | null
    profile_data: Record<string, unknown>
    created_at: Date
    updated_at: Date
}

export interface UserOAuthCredential {
    id: string
    user_id: string
    provider: string
    provider_user_id: string
    access_token?: string
    refresh_token?: string
    token_type: string
    expires_at?: Date
    scopes?: string[]
    profile_data: Record<string, any>
    created_at: Date
    updated_at: Date
}

export class UserOAuthCredentialsService {
    static async saveCredentials(
        userId: string,
        provider: string,
        profile: OAuthProfile,
        tokens: OAuthTokens,
    ): Promise<UserOAuthCredential> {
        const id = ulid()
        const expiresAt = tokens.expires_in
            ? new Date(Date.now() + tokens.expires_in * 1000).toISOString()
            : null

        const scopes = tokens.scope ? tokens.scope.split(' ') : []

        await db.execute(sql`
            INSERT INTO user_oauth_credentials (
                id, user_id, provider, provider_user_id,
                access_token, refresh_token, token_type,
                expires_at, scopes, profile_data
            ) VALUES (
                ${id}, ${userId}, ${provider}, ${profile.id},
                ${tokens.access_token}, ${tokens.refresh_token || null}, ${tokens.token_type},
                ${expiresAt}, ${`{${scopes.join(',')}}`}, ${JSON.stringify(profile)}
            )
            ON CONFLICT (user_id, provider, provider_user_id) 
            DO UPDATE SET
                access_token = EXCLUDED.access_token,
                refresh_token = COALESCE(EXCLUDED.refresh_token, user_oauth_credentials.refresh_token),
                token_type = EXCLUDED.token_type,
                expires_at = EXCLUDED.expires_at,
                scopes = EXCLUDED.scopes,
                profile_data = EXCLUDED.profile_data,
                updated_at = NOW()
        `)

        return this.getCredentials(userId, provider, profile.id)
    }

    static async getCredentials(
        userId: string,
        provider: string,
        providerUserId: string,
    ): Promise<UserOAuthCredential> {
        const rows = await db.execute(sql`
            SELECT * FROM user_oauth_credentials
            WHERE user_id = ${userId}
            AND provider = ${provider}
            AND provider_user_id = ${providerUserId}
            LIMIT 1
        `)

        if (!rows.length) {
            throw new Error('OAuth credentials not found')
        }

        const r = rows[0] as UserOAuthCredentialRow
        return {
            id: r.id,
            user_id: r.user_id,
            provider: r.provider,
            provider_user_id: r.provider_user_id,
            access_token: r.access_token ?? undefined,
            refresh_token: r.refresh_token ?? undefined,
            token_type: r.token_type,
            expires_at: r.expires_at ?? undefined,
            scopes: r.scopes ?? [],
            profile_data: r.profile_data ?? {},
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }

    static async getUserOAuthCredentials(userId: string): Promise<UserOAuthCredential[]> {
        const rows = await db.execute(sql`
            SELECT * FROM user_oauth_credentials
            WHERE user_id = ${userId}
            ORDER BY provider, created_at
        `)

        return rows.map((row) => {
            const r = row as UserOAuthCredentialRow
            return {
                id: r.id,
                user_id: r.user_id,
                provider: r.provider,
                provider_user_id: r.provider_user_id,
                access_token: r.access_token ?? undefined,
                refresh_token: r.refresh_token ?? undefined,
                token_type: r.token_type,
                expires_at: r.expires_at ?? undefined,
                scopes: r.scopes ?? [],
                profile_data: r.profile_data ?? {},
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
    }

    static async findByProviderProfile(
        provider: string,
        providerUserId: string,
    ): Promise<UserOAuthCredential | null> {
        const rows = await db.execute(sql`
            SELECT * FROM user_oauth_credentials
            WHERE provider = ${provider}
            AND provider_user_id = ${providerUserId}
            LIMIT 1
        `)

        if (!rows.length) {
            return null
        }

        const r = rows[0] as UserOAuthCredentialRow
        return {
            id: r.id,
            user_id: r.user_id,
            provider: r.provider,
            provider_user_id: r.provider_user_id,
            access_token: r.access_token ?? undefined,
            refresh_token: r.refresh_token ?? undefined,
            token_type: r.token_type,
            expires_at: r.expires_at ?? undefined,
            scopes: r.scopes ?? [],
            profile_data: r.profile_data ?? {},
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }

    static async removeCredentials(
        userId: string,
        provider: string,
        providerUserId: string,
    ): Promise<void> {
        await db.execute(sql`
            DELETE FROM user_oauth_credentials 
            WHERE user_id = ${userId} 
            AND provider = ${provider} 
            AND provider_user_id = ${providerUserId}
        `)
    }

    static async updateTokens(
        userId: string,
        provider: string,
        providerUserId: string,
        tokens: OAuthTokens,
    ): Promise<void> {
        const expiresAt = tokens.expires_in
            ? new Date(Date.now() + tokens.expires_in * 1000).toISOString()
            : null

        await db.execute(sql`
            UPDATE user_oauth_credentials
            SET
                access_token = ${tokens.access_token},
                refresh_token = COALESCE(${tokens.refresh_token}, refresh_token),
                token_type = ${tokens.token_type},
                expires_at = ${expiresAt},
                updated_at = NOW()
            WHERE user_id = ${userId} 
            AND provider = ${provider} 
            AND provider_user_id = ${providerUserId}
        `)
    }
}
