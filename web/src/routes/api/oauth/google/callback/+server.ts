import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getRedisClient } from '$lib/server/redis'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { oauth } from '$lib/server/config'
import crypto from 'crypto'

const GOOGLE_TOKEN_URL = 'https://oauth2.googleapis.com/token'

export const GET: RequestHandler = async ({ url, locals }) => {
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const errorParam = url.searchParams.get('error')

    if (errorParam) {
        throw redirect(302, '/admin/integrations?error=oauth_denied')
    }

    if (!code || !state) {
        throw error(400, 'Missing required parameters')
    }

    const redis = await getRedisClient()
    const stateData = await redis.get(`oauth:state:${state}`)
    await redis.del(`oauth:state:${state}`)
    await redis.quit()

    if (!stateData) {
        throw error(400, 'Invalid or expired state')
    }

    const { userId, isAdmin } = JSON.parse(stateData)

    if (!locals.user || locals.user.id !== userId) {
        throw error(401, 'Unauthorized')
    }

    // Only admins can complete org-level OAuth flows
    if (!isAdmin || locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    try {
        const tokenResponse = await fetch(GOOGLE_TOKEN_URL, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
            },
            body: new URLSearchParams({
                code,
                client_id: oauth.google.clientId,
                client_secret: oauth.google.clientSecret,
                redirect_uri: oauth.google.redirectUri,
                grant_type: 'authorization_code',
            }),
        })

        if (!tokenResponse.ok) {
            const errorData = await tokenResponse.json()
            console.error('Token exchange failed:', errorData)
            throw error(500, 'Failed to exchange authorization code')
        }

        const tokens = await tokenResponse.json()

        const userInfoResponse = await fetch('https://www.googleapis.com/oauth2/v2/userinfo', {
            headers: {
                Authorization: `Bearer ${tokens.access_token}`,
            },
        })

        if (!userInfoResponse.ok) {
            throw error(500, 'Failed to fetch user info')
        }

        const userInfo = await userInfoResponse.json()

        const encryptedTokens = {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            token_type: tokens.token_type,
            expires_in: tokens.expires_in,
            obtained_at: Date.now(),
        }

        // Check for existing org-level Google connection
        const existingSource = await db.query.sources.findFirst({
            where: eq(sources.sourceType, 'google'),
        })

        if (existingSource) {
            // Update existing org-level connection
            await db
                .update(sources)
                .set({
                    oauthCredentials: encryptedTokens,
                    config: {
                        email: userInfo.email,
                        name: userInfo.name,
                    },
                    syncStatus: 'completed',
                    isActive: true,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, existingSource.id))
        } else {
            // Create new org-level Google connection
            const sourceId = crypto.randomBytes(13).toString('hex')
            await db.insert(sources).values({
                id: sourceId,
                createdBy: userId,
                name: `Google Workspace`,
                sourceType: 'google',
                config: {
                    email: userInfo.email,
                    name: userInfo.name,
                },
                oauthCredentials: encryptedTokens,
                syncStatus: 'completed',
                isActive: true,
            })
        }

        throw redirect(302, '/admin/integrations?success=google_connected')
    } catch (err) {
        console.error('OAuth callback error:', err)
        if (err instanceof Response) throw err
        throw error(500, 'OAuth callback failed')
    }
}
