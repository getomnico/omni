import { redirect, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { GoogleConnectorOAuthService } from '$lib/server/oauth/googleConnector'
import { logger } from '$lib/server/logger'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { ulid } from 'ulid'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'

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

        for (const serviceType of serviceTypes) {
            // If an org-wide source of this type exists, attach the user's tokens
            // to it as a per-user credential rather than creating a new personal
            // source. This is the "attach to org source" path from the plan: it
            // means we never end up with both an org and a personal source for the
            // same provider for the same user.
            const [orgSource] = await db
                .select({ id: sources.id })
                .from(sources)
                .where(
                    and(
                        eq(sources.sourceType, serviceType),
                        eq(sources.scope, 'org'),
                        eq(sources.isDeleted, false),
                    ),
                )
                .limit(1)

            if (orgSource) {
                const expiresAt = tokens.expires_in
                    ? new Date(Date.now() + tokens.expires_in * 1000)
                    : null
                const grantedScopes = (tokens.scope ?? '').split(' ').filter(Boolean)

                await serviceCredentialsRepository.createForUser({
                    sourceId: orgSource.id,
                    userId: locals.user.id,
                    provider: 'google',
                    authType: 'oauth',
                    principalEmail: userEmail,
                    credentials,
                    config: { granted_scopes: grantedScopes },
                    expiresAt,
                })

                logger.info(
                    `Attached per-user OAuth creds to org source ${orgSource.id} (${serviceType}) for user ${locals.user.id}`,
                )
                continue
            }

            // No org source — fall through to the existing personal-source flow.
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

            const [newSource] = await db
                .insert(sources)
                .values({
                    id: ulid(),
                    name: SOURCE_NAMES[serviceType] || serviceType,
                    sourceType: serviceType,
                    scope: 'user',
                    config: {},
                    createdBy: locals.user.id,
                    isActive: true,
                })
                .returning()

            const credResponse = await svelteFetch('/api/service-credentials', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    sourceId: newSource.id,
                    provider: 'google',
                    authType: 'oauth',
                    principalEmail: userEmail,
                    credentials,
                    config: {},
                }),
            })

            if (!credResponse.ok) {
                const errText = await credResponse.text()
                logger.error(
                    `Failed to store OAuth credentials for source ${newSource.id}:`,
                    errText,
                )
                throw new Error(`Failed to store credentials for ${serviceType}`)
            }

            logger.info(`Stored OAuth credentials for source ${newSource.id} (${serviceType})`)
        }
    } catch (err: any) {
        if (err?.status === 302) throw err // re-throw redirects
        logger.error('Google OAuth callback error:', err)
        throw redirect(302, '/settings/integrations?error=oauth_failed')
    }

    // Redirect back to integrations page
    throw redirect(302, '/settings/integrations?success=google_connected')
}
