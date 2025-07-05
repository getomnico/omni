import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'
import { ServiceProvider, AuthType } from '$lib/types'
import { ulid } from 'ulid'
import { INDEXER_URL } from '$env/static/private'

export const POST: RequestHandler = async ({ request, locals, fetch }) => {
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
        // Call indexer service to create encrypted credentials
        const indexerResponse = await fetch(`${INDEXER_URL}/service-credentials`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                source_id: sourceId,
                provider: provider,
                auth_type: authType,
                principal_email: principalEmail || null,
                credentials: credentials,
                config: config || {},
            }),
        })

        if (!indexerResponse.ok) {
            const errorText = await indexerResponse.text()
            console.error('Indexer service error:', errorText)
            throw error(500, 'Failed to create service credentials')
        }

        const indexerResult = await indexerResponse.json()

        if (!indexerResult.success) {
            console.error('Indexer service failed:', indexerResult.message)
            throw error(500, indexerResult.message || 'Failed to create service credentials')
        }

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
                id: ulid(), // Generate a temporary ID for response
                sourceId: sourceId,
                provider: provider,
                authType: authType,
                principalEmail: principalEmail || null,
                config: config || {},
                expiresAt: null,
                lastValidatedAt: null,
                createdAt: new Date().toISOString(),
                updatedAt: new Date().toISOString(),
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
