import { db } from '../db'
import { sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { randomBytes } from 'crypto'
import type { OAuthState } from './types'

export class OAuthStateManager {
    private static readonly STATE_EXPIRY_MINUTES = 15

    static async createState(
        provider: string,
        redirectUri?: string,
        userId?: string,
        metadata: Record<string, any> = {},
    ): Promise<{ stateToken: string; nonce: string }> {
        const stateToken = randomBytes(32).toString('hex')
        const nonce = randomBytes(16).toString('hex')
        const id = ulid()
        const expiresAt = new Date(Date.now() + this.STATE_EXPIRY_MINUTES * 60 * 1000)

        await db.execute(sql`
            INSERT INTO oauth_state (id, state_token, provider, redirect_uri, nonce, user_id, expires_at, metadata)
            VALUES (${id}, ${stateToken}, ${provider}, ${redirectUri || null}, ${nonce}, ${userId || null}, ${expiresAt}, ${JSON.stringify(metadata)})
        `)

        return { stateToken, nonce }
    }

    static async validateAndConsumeState(stateToken: string): Promise<OAuthState | null> {
        const result = await db.execute(sql`
            SELECT * FROM oauth_state 
            WHERE state_token = ${stateToken} 
            AND expires_at > NOW()
            LIMIT 1
        `)

        if (!result.rows.length) {
            return null
        }

        const row = result.rows[0] as any
        const state: OAuthState = {
            id: row.id,
            state_token: row.state_token,
            provider: row.provider,
            redirect_uri: row.redirect_uri,
            nonce: row.nonce,
            code_verifier: row.code_verifier,
            user_id: row.user_id,
            expires_at: row.expires_at,
            created_at: row.created_at,
            metadata: row.metadata || {},
        }

        // Delete the state token to prevent replay attacks
        await db.execute(sql`
            DELETE FROM oauth_state WHERE state_token = ${stateToken}
        `)

        return state
    }

    static async cleanupExpiredStates(): Promise<void> {
        await db.execute(sql`
            DELETE FROM oauth_state WHERE expires_at < NOW()
        `)
    }
}
