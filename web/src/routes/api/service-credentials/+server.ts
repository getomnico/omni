import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { ServiceProvider, AuthType } from '$lib/types'
import { ulid } from 'ulid'

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
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

    // Check if source exists and is owned by requesting user or user is admin
    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    try {
        // Delete existing credentials for this source
        await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

        // Create new credentials
        const newCredentials = await db
            .insert(serviceCredentials)
            .values({
                id: ulid(),
                sourceId: sourceId,
                provider: provider,
                authType: authType,
                principalEmail: principalEmail || null,
                credentials: credentials,
                config: config || {},
            })
            .returning()

        return json({
            success: true,
            credentials: {
                id: newCredentials[0].id,
                sourceId: newCredentials[0].sourceId,
                provider: newCredentials[0].provider,
                authType: newCredentials[0].authType,
                principalEmail: newCredentials[0].principalEmail,
                config: newCredentials[0].config,
                expiresAt: newCredentials[0].expiresAt,
                lastValidatedAt: newCredentials[0].lastValidatedAt,
                createdAt: newCredentials[0].createdAt,
                updatedAt: newCredentials[0].updatedAt,
                // Don't return sensitive credentials
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
            return json({ credentials: null })
        }

        return json({
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
