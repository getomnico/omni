import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { ServiceProvider, AuthType } from '$lib/types'
import { ulid } from 'ulid'
import { encryptConfig } from '$lib/server/crypto/encryption'

export const POST: RequestHandler = async ({ request, locals, fetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const { sourceId, provider, authType, principalEmail, credentials, config } =
        await request.json()

    // Validate required fields
    if (!sourceId || !provider || !authType || !credentials) {
        throw error(400, 'Missing required fields')
    }

    // Validate provider and auth type
    if (!Object.values(ServiceProvider).includes(provider)) {
        throw error(400, 'Invalid provider')
    }

    if (!Object.values(AuthType).includes(authType)) {
        throw error(400, 'Invalid auth type')
    }

    // Check if source exists
    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    // Allow source owner in addition to admins (e.g. OAuth callback for non-admin users)
    const isOwner = source.createdBy === locals.user.id
    if (locals.user.role !== 'admin' && !isOwner) {
        throw error(403, 'Forbidden')
    }

    try {
        // Delete existing credentials for this source
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

        // Encrypt and insert new credentials directly
        const id = ulid()
        const encryptedCredentials = encryptConfig(credentials)

        const [created] = await db
            .insert(serviceCredentials)
            .values({
                id,
                sourceId,
                provider,
                authType,
                principalEmail: principalEmail || null,
                credentials: encryptedCredentials,
                config: config || {},
            })
            .returning()

        // Trigger initial sync after credentials are saved
        try {
            const syncResponse = await fetch(`/api/sources/${sourceId}/sync`, {
                method: 'POST',
            })

            if (!syncResponse.ok) {
                console.warn(
                    `Failed to trigger initial sync for source ${sourceId}:`,
                    await syncResponse.text(),
                )
            }
        } catch (syncError) {
            console.warn(`Error triggering initial sync for source ${sourceId}:`, syncError)
        }

        return json({
            success: true,
            credentials: {
                id: created.id,
                sourceId: created.sourceId,
                provider: created.provider,
                authType: created.authType,
                principalEmail: created.principalEmail,
                config: created.config,
                expiresAt: created.expiresAt,
                lastValidatedAt: created.lastValidatedAt,
                createdAt: created.createdAt,
                updatedAt: created.updatedAt,
            },
        })
    } catch (err) {
        console.error('Error creating service credentials:', err)
        throw error(500, 'Failed to create service credentials')
    }
}

export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const sourceId = url.searchParams.get('sourceId')

    if (!sourceId) {
        throw error(400, 'Missing sourceId parameter')
    }

    try {
        const creds = await db.query.serviceCredentials.findFirst({
            where: eq(serviceCredentials.sourceId, sourceId),
        })

        if (!creds) {
            return json({ credentials: null, hasCredentials: false })
        }

        return json({
            hasCredentials: true,
            credentials: {
                id: creds.id,
                sourceId: creds.sourceId,
                provider: creds.provider,
                authType: creds.authType,
                principalEmail: creds.principalEmail,
                config: creds.config,
                expiresAt: creds.expiresAt,
                lastValidatedAt: creds.lastValidatedAt,
                createdAt: creds.createdAt,
                updatedAt: creds.updatedAt,
                // Don't return sensitive credentials
            },
        })
    } catch (err) {
        console.error('Error fetching service credentials:', err)
        throw error(500, 'Failed to fetch service credentials')
    }
}

export const PATCH: RequestHandler = async ({ request, locals, fetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const { sourceId, principalEmail, credentials, config } = await request.json()

    if (!sourceId) {
        throw error(400, 'Missing sourceId')
    }

    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })
    if (!source) {
        throw error(404, 'Source not found')
    }

    const isOwner = source.createdBy === locals.user.id
    if (locals.user.role !== 'admin' && !isOwner) {
        throw error(403, 'Forbidden')
    }

    const existing = await db.query.serviceCredentials.findFirst({
        where: eq(serviceCredentials.sourceId, sourceId),
    })
    if (!existing) {
        throw error(404, 'No service credentials exist for this source')
    }

    const hasNewCredentials =
        credentials && typeof credentials === 'object' && Object.keys(credentials).length > 0

    const updates: Partial<typeof serviceCredentials.$inferInsert> = {
        updatedAt: new Date(),
    }

    if (principalEmail !== undefined) {
        updates.principalEmail = principalEmail || null
    }
    if (config !== undefined) {
        updates.config = config || {}
    }
    if (hasNewCredentials) {
        updates.credentials = encryptConfig(credentials)
    }

    try {
        const [updated] = await db
            .update(serviceCredentials)
            .set(updates)
            .where(eq(serviceCredentials.sourceId, sourceId))
            .returning()

        if (hasNewCredentials) {
            try {
                const syncResponse = await fetch(`/api/sources/${sourceId}/sync`, {
                    method: 'POST',
                })
                if (!syncResponse.ok) {
                    console.warn(
                        `Failed to trigger sync after credential update for source ${sourceId}:`,
                        await syncResponse.text(),
                    )
                }
            } catch (syncError) {
                console.warn(
                    `Error triggering sync after credential update for source ${sourceId}:`,
                    syncError,
                )
            }
        }

        return json({
            success: true,
            credentials: {
                id: updated.id,
                sourceId: updated.sourceId,
                provider: updated.provider,
                authType: updated.authType,
                principalEmail: updated.principalEmail,
                config: updated.config,
                expiresAt: updated.expiresAt,
                lastValidatedAt: updated.lastValidatedAt,
                createdAt: updated.createdAt,
                updatedAt: updated.updatedAt,
            },
        })
    } catch (err) {
        console.error('Error updating service credentials:', err)
        throw error(500, 'Failed to update service credentials')
    }
}

export const DELETE: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const sourceId = url.searchParams.get('sourceId')

    if (!sourceId) {
        throw error(400, 'Missing sourceId parameter')
    }

    try {
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

        return json({ success: true })
    } catch (err) {
        console.error('Error deleting service credentials:', err)
        throw error(500, 'Failed to delete service credentials')
    }
}
