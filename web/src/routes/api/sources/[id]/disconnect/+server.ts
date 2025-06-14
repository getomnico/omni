import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'

export const POST: RequestHandler = async ({ params, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = params.id

    const source = await db.query.sources.findFirst({
        where: and(eq(sources.id, sourceId), eq(sources.createdBy, locals.user.id)),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.oauthCredentials && source.sourceType === 'google_drive') {
        try {
            const credentials = source.oauthCredentials as any
            if (credentials.access_token) {
                await fetch(
                    `https://oauth2.googleapis.com/revoke?token=${credentials.access_token}`,
                    {
                        method: 'POST',
                    },
                )
            }
        } catch (err) {
            console.error('Failed to revoke Google token:', err)
        }
    }

    await db
        .update(sources)
        .set({
            oauthCredentials: null,
            syncStatus: 'pending',
            isActive: false,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))

    return json({ success: true })
}
