import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getRedisClient } from '$lib/server/redis'
import { db } from '$lib/server/db'
import { sources, oauthCredentials } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { oauth } from '$lib/server/config'
import { SourceType, OAuthProvider } from '$lib/types'
import crypto from 'crypto'
import { ulid } from 'ulid'

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

        // Calculate token expiration time
        const expiresAt = new Date(Date.now() + tokens.expires_in * 1000)

        // Check for existing org-level Google connection
        const existingSource = await db.query.sources.findFirst({
            where: eq(sources.sourceType, SourceType.GOOGLE),
        })

        let sourceId: string

        if (existingSource) {
            sourceId = existingSource.id
            
            // Update existing org-level connection
            await db
                .update(sources)
                .set({
                    config: {
                        email: userInfo.email,
                        name: userInfo.name,
                    },
                    syncStatus: 'completed',
                    isActive: true,
                    updatedAt: new Date(),
                })
                .where(eq(sources.id, existingSource.id))

            // Delete existing OAuth credentials for this source
            await db
                .delete(oauthCredentials)
                .where(eq(oauthCredentials.sourceId, sourceId))
        } else {
            // Create new org-level Google connection
            sourceId = ulid()
            await db.insert(sources).values({
                id: sourceId,
                createdBy: userId,
                name: `Google Workspace`,
                sourceType: SourceType.GOOGLE,
                config: {
                    email: userInfo.email,
                    name: userInfo.name,
                },
                syncStatus: 'completed',
                isActive: true,
            })
        }

        // Save OAuth credentials in the oauth_credentials table
        await db.insert(oauthCredentials).values({
            id: ulid(),
            sourceId: sourceId,
            provider: OAuthProvider.GOOGLE,
            clientId: oauth.google.clientId,
            clientSecret: oauth.google.clientSecret,
            accessToken: tokens.access_token,
            refreshToken: tokens.refresh_token,
            tokenType: tokens.token_type,
            expiresAt: expiresAt,
            metadata: {
                email: userInfo.email,
                name: userInfo.name,
                obtained_at: Date.now(),
            },
        })

    } catch (err) {
        console.error('OAuth callback error:', err)
        throw error(500, 'OAuth callback failed')
    }

    throw redirect(302, '/admin/integrations?success=google_connected')
}
