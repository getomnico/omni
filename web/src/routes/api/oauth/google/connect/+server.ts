import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getRedisClient } from '$lib/server/redis'
import { oauth } from '$lib/server/config'
import crypto from 'crypto'

const GOOGLE_AUTH_URL = 'https://accounts.google.com/o/oauth2/v2/auth'
const SCOPES = [
    'https://www.googleapis.com/auth/drive.readonly',
    'https://www.googleapis.com/auth/userinfo.email',
    'https://www.googleapis.com/auth/userinfo.profile',
]

export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const state = crypto.randomBytes(32).toString('hex')

    const redis = await getRedisClient()
    await redis.setEx(
        `oauth:state:${state}`,
        300,
        JSON.stringify({
            userId: locals.user.id,
            timestamp: Date.now(),
        }),
    )
    await redis.quit()

    const params = new URLSearchParams({
        client_id: oauth.google.clientId,
        redirect_uri: oauth.google.redirectUri,
        response_type: 'code',
        scope: SCOPES.join(' '),
        state,
        access_type: 'offline',
        prompt: 'consent',
    })

    throw redirect(302, `${GOOGLE_AUTH_URL}?${params}`)
}
