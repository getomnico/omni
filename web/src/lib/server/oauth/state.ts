import { getRedisClient } from '../redis'
import { randomBytes } from 'crypto'
import type { OAuthState } from './types'

const STATE_PREFIX = 'oauth_state'

export class OAuthStateManager {
    private static readonly STATE_EXPIRY_SECONDS = 15 * 60

    static async createState(
        provider: string,
        redirectUri?: string,
        userId?: string,
        metadata: Record<string, any> = {},
    ): Promise<{ stateToken: string; nonce: string }> {
        const stateToken = randomBytes(32).toString('hex')
        const nonce = randomBytes(16).toString('hex')
        const now = new Date()

        const state: OAuthState = {
            id: stateToken,
            state_token: stateToken,
            provider,
            redirect_uri: redirectUri,
            nonce,
            user_id: userId,
            expires_at: new Date(now.getTime() + this.STATE_EXPIRY_SECONDS * 1000),
            created_at: now,
            metadata,
        }

        const redis = await getRedisClient()
        await redis.setEx(
            `${STATE_PREFIX}:${stateToken}`,
            this.STATE_EXPIRY_SECONDS,
            JSON.stringify(state),
        )

        return { stateToken, nonce }
    }

    static async validateAndConsumeState(stateToken: string): Promise<OAuthState | null> {
        const redis = await getRedisClient()
        const key = `${STATE_PREFIX}:${stateToken}`

        const data = await redis.get(key)
        if (!data) {
            return null
        }

        // Delete immediately to prevent replay attacks
        await redis.del(key)

        return JSON.parse(data) as OAuthState
    }
}
