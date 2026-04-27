import { db } from '../db'
import { sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import type { OAuthTokens, OAuthProfile } from './types'
import { encrypt, decrypt, type EncryptedData } from '../crypto/encryption'

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

interface EncryptedTokenJsonb {
    encrypted_data: EncryptedData
    version: number
}

function encryptToken(token: string | null | undefined): EncryptedTokenJsonb | null {
    if (!token) return null
    return { encrypted_data: encrypt(token), version: 1 }
}

function decryptToken(dbValue: unknown): string | undefined {
    if (dbValue === null || dbValue === undefined) return undefined
    const obj = dbValue as Record<string, unknown>
    if (!obj.encrypted_data || typeof obj.encrypted_data !== 'object') return undefined
    return decrypt(obj.encrypted_data as EncryptedData)
}

function rowToCredential(row: any): UserOAuthCredential {
    return {
        id: row.id,
        user_id: row.user_id,
        provider: row.provider,
        provider_user_id: row.provider_user_id,
        access_token: decryptToken(row.access_token),
        refresh_token: decryptToken(row.refresh_token),
        token_type: row.token_type,
        expires_at: row.expires_at,
        scopes: row.scopes || [],
        profile_data: row.profile_data || {},
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
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
        const accessTokenJson = JSON.stringify(encryptToken(tokens.access_token))
        const refreshTokenJson = JSON.stringify(encryptToken(tokens.refresh_token))

        await db.execute(sql`
            INSERT INTO user_oauth_credentials (
                id, user_id, provider, provider_user_id,
                access_token, refresh_token, token_type,
                expires_at, scopes, profile_data
            ) VALUES (
                ${id}, ${userId}, ${provider}, ${profile.id},
                ${accessTokenJson}::jsonb, ${refreshTokenJson}::jsonb, ${tokens.token_type},
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

        return rowToCredential(rows[0])
    }

    static async getUserOAuthCredentials(userId: string): Promise<UserOAuthCredential[]> {
        const rows = await db.execute(sql`
            SELECT * FROM user_oauth_credentials
            WHERE user_id = ${userId}
            ORDER BY provider, created_at
        `)

        return rows.map(rowToCredential)
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

        return rowToCredential(rows[0])
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

        // Refresh tokens are often omitted on token-refresh responses; in that
        // case we leave the stored encrypted refresh_token untouched.
        const accessTokenJson = JSON.stringify(encryptToken(tokens.access_token))
        const refreshTokenJson = tokens.refresh_token
            ? JSON.stringify(encryptToken(tokens.refresh_token))
            : null

        await db.execute(sql`
            UPDATE user_oauth_credentials
            SET
                access_token = ${accessTokenJson}::jsonb,
                refresh_token = COALESCE(${refreshTokenJson}::jsonb, refresh_token),
                token_type = ${tokens.token_type},
                expires_at = ${expiresAt},
                updated_at = NOW()
            WHERE user_id = ${userId}
            AND provider = ${provider}
            AND provider_user_id = ${providerUserId}
        `)
    }
}
