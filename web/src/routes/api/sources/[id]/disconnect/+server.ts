import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, oauthCredentials } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { SourceType } from '$lib/types'

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

    // Get OAuth credentials for this source
    const credentials = await db.query.oauthCredentials.findFirst({
        where: eq(oauthCredentials.sourceId, sourceId),
    })

    if (credentials && source.sourceType === SourceType.GOOGLE) {
        try {
            if (credentials.accessToken) {
                await fetch(
                    `https://oauth2.googleapis.com/revoke?token=${credentials.accessToken}`,
                    {
                        method: 'POST',
                    },
                )
            }
        } catch (err) {
            console.error('Failed to revoke Google token:', err)
        }
    }

    // Delete OAuth credentials
    if (credentials) {
        await db
            .delete(oauthCredentials)
            .where(eq(oauthCredentials.sourceId, sourceId))
    }

    // Update source status
    await db
        .update(sources)
        .set({
            syncStatus: 'pending',
            isActive: false,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))

    return json({ success: true })
}
