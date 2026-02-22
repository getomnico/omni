import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { GoogleConnectorOAuthService } from '$lib/server/oauth/googleConnector'
import { env } from '$env/dynamic/private'
import { logger } from '$lib/server/logger'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { ulid } from 'ulid'

const SOURCE_NAMES: Record<string, string> = {
    google_drive: 'Google Drive (OAuth)',
    gmail: 'Gmail (OAuth)',
}

export const GET: RequestHandler = async ({ url, locals, fetch: svelteFetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const code = url.searchParams.get('code')
    const stateToken = url.searchParams.get('state')
    const oauthError = url.searchParams.get('error')

    if (oauthError) {
        logger.error('Google OAuth error:', oauthError)
        throw redirect(302, '/settings/integrations?error=oauth_denied')
    }

    if (!code || !stateToken) {
        throw error(400, 'Missing code or state parameter')
    }

    try {
        // Exchange code for tokens
        const { tokens, state } = await GoogleConnectorOAuthService.exchangeCodeForTokens(
            code,
            stateToken,
        )

        const serviceTypes: string[] = state.metadata?.serviceTypes
        if (!serviceTypes || serviceTypes.length === 0) {
            throw new Error('Missing serviceTypes in OAuth state')
        }

        // Fetch user email from Google
        const userEmail = await GoogleConnectorOAuthService.fetchUserEmail(tokens.access_token)

        // Calculate token expiry timestamp
        const expiresAt = tokens.expires_in ? Math.floor(Date.now() / 1000) + tokens.expires_in : 0

        const credentials = {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: expiresAt,
            user_email: userEmail,
        }

        // Create sources and store credentials for each service type
        for (const serviceType of serviceTypes) {
            // Check for existing source (duplicate guard)
            const [existing] = await db
                .select({ id: sources.id })
                .from(sources)
                .where(
                    and(
                        eq(sources.sourceType, serviceType),
                        eq(sources.createdBy, locals.user.id),
                        eq(sources.isDeleted, false),
                    ),
                )
                .limit(1)

            if (existing) {
                logger.info(
                    `Skipping ${serviceType} source creation — already exists for user ${locals.user.id}`,
                )
                continue
            }

            // Create source
            const [newSource] = await db
                .insert(sources)
                .values({
                    id: ulid(),
                    name: SOURCE_NAMES[serviceType] || serviceType,
                    sourceType: serviceType,
                    config: {},
                    createdBy: locals.user.id,
                    isActive: true,
                })
                .returning()

            // Store credentials via indexer
            const indexerResponse = await svelteFetch(`${env.INDEXER_URL}/service-credentials`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    source_id: newSource.id,
                    provider: 'google',
                    auth_type: 'oauth',
                    principal_email: userEmail,
                    credentials,
                    config: {},
                }),
            })

            if (!indexerResponse.ok) {
                const errorText = await indexerResponse.text()
                logger.error(
                    `Failed to store OAuth credentials for source ${newSource.id}:`,
                    errorText,
                )
                throw new Error(`Failed to store credentials for ${serviceType}`)
            }

            // Trigger initial sync
            try {
                await svelteFetch(`/api/sources/${newSource.id}/sync`, { method: 'POST' })
            } catch (syncError) {
                logger.warn(`Failed to trigger initial sync for source ${newSource.id}:`, syncError)
            }
        }
    } catch (err: any) {
        if (err?.status === 302) throw err // re-throw redirects
        logger.error('Google OAuth callback error:', err)
        throw redirect(302, '/settings/integrations?error=oauth_failed')
    }

    // Redirect back to integrations page
    throw redirect(302, '/settings/integrations?success=google_connected')
}
