import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { and, eq, isNull } from 'drizzle-orm'
import { ulid } from 'ulid'
import { db } from '$lib/server/db'
import { encryptConfig } from '$lib/server/crypto/encryption'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { getSourceById } from '$lib/server/db/sources'
import {
    getOAuthManifestForSourceType,
    requestOAuthCredentialValidation,
    SOURCE_BINDING_CONFIG_KEY,
    type OAuthSourceBinding,
} from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { ServiceProvider, AuthType, IntegrationType, supportsDataSync } from '$lib/types'

/**
 * Validate an org credential and derive its admin-owned source binding for
 * connectors that declare an OAuth manifest. Persistence happens later in the
 * same transaction as the credential replacement.
 */
async function validateOrgSourceBinding(
    sourceId: string,
    sourceType: string,
    provider: string,
    credentials: Record<string, unknown>,
): Promise<OAuthSourceBinding | null> {
    const manifest = await getOAuthManifestForSourceType(sourceType)
    if (!manifest) return null
    return requestOAuthCredentialValidation({
        sourceId,
        provider,
        credentials,
        flow: 'org_source',
    })
}

export const POST: RequestHandler = async ({ request, locals, fetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const {
        sourceId,
        provider,
        authType,
        principalEmail,
        credentials,
        config,
        triggerSync = true,
    } = await request.json()

    if (!sourceId || !provider || !authType || !credentials) {
        throw error(400, 'Missing required fields')
    }

    if (!Object.values(ServiceProvider).includes(provider)) {
        throw error(400, 'Invalid provider')
    }

    if (!Object.values(AuthType).includes(authType)) {
        throw error(400, 'Invalid auth type')
    }

    const source = await getSourceById(sourceId)
    if (!source) {
        throw error(404, 'Source not found')
    }

    const isOwner = source.createdBy === locals.user.id
    if (locals.user.role !== 'admin' && !isOwner) {
        throw error(403, 'Forbidden')
    }

    // Validate before opening the transaction so a remote connector call does
    // not hold database locks. A rejected credential must not be persisted.
    let orgSourceBinding: OAuthSourceBinding | null = null
    if (source.scope === 'org' && source.integrationType === IntegrationType.CONNECTOR) {
        try {
            orgSourceBinding = await validateOrgSourceBinding(
                sourceId,
                source.sourceType,
                provider,
                credentials as Record<string, unknown>,
            )
        } catch (bindingError) {
            throw error(
                400,
                bindingError instanceof Error ? bindingError.message : 'OAuth credential rejected',
            )
        }
    }

    try {
        // Personal-source creds belong to the source's owner (per-user row).
        // Org-source creds and their source binding are replaced atomically.
        const created =
            source.scope === 'user'
                ? await serviceCredentialsRepository.createForUser({
                      sourceId,
                      userId: source.createdBy,
                      provider,
                      authType,
                      principalEmail: principalEmail || null,
                      credentials,
                      config: config || {},
                  })
                : await db.transaction(async (tx) => {
                      if (orgSourceBinding && Object.keys(orgSourceBinding).length > 0) {
                          await tx
                              .update(sources)
                              .set({
                                  config: {
                                      ...((source.config ?? {}) as Record<string, unknown>),
                                      [SOURCE_BINDING_CONFIG_KEY]: orgSourceBinding,
                                  },
                                  updatedAt: new Date(),
                              })
                              .where(eq(sources.id, sourceId))
                      }
                      await tx
                          .delete(serviceCredentials)
                          .where(
                              and(
                                  eq(serviceCredentials.sourceId, sourceId),
                                  isNull(serviceCredentials.userId),
                              ),
                          )
                      const [credential] = await tx
                          .insert(serviceCredentials)
                          .values({
                              id: ulid(),
                              sourceId,
                              userId: null,
                              provider,
                              authType,
                              principalEmail: principalEmail || null,
                              credentials: encryptConfig(credentials),
                              config: config || {},
                          })
                          .returning()
                      return credential
                  })

        if (triggerSync && supportsDataSync(source.integrationType)) {
            try {
                const syncResponse = await fetch(`/api/sources/${sourceId}/sync`, {
                    method: 'POST',
                    headers: { 'content-type': 'application/json' },
                    body: JSON.stringify({}),
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
        const creds = await serviceCredentialsRepository.getOrgCredsBySourceId(sourceId)

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

    const source = await getSourceById(sourceId)
    if (!source) {
        throw error(404, 'Source not found')
    }

    const isOwner = source.createdBy === locals.user.id
    if (locals.user.role !== 'admin' && !isOwner) {
        throw error(403, 'Forbidden')
    }

    const existing = await serviceCredentialsRepository.getOrgCredsBySourceId(sourceId)
    if (!existing) {
        throw error(404, 'No service credentials exist for this source')
    }

    const hasNewCredentials =
        credentials && typeof credentials === 'object' && Object.keys(credentials).length > 0

    try {
        const updated = await serviceCredentialsRepository.updateBySourceId(sourceId, {
            principalEmail: principalEmail !== undefined ? principalEmail || null : undefined,
            config: config !== undefined ? config || {} : undefined,
            credentials: hasNewCredentials ? credentials : null,
        })

        if (hasNewCredentials && supportsDataSync(source.integrationType)) {
            try {
                const syncResponse = await fetch(`/api/sources/${sourceId}/sync`, {
                    method: 'POST',
                    headers: { 'content-type': 'application/json' },
                    body: JSON.stringify({}),
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
            credentials: updated && {
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
        await serviceCredentialsRepository.deleteBySourceId(sourceId)
        return json({ success: true })
    } catch (err) {
        console.error('Error deleting service credentials:', err)
        throw error(500, 'Failed to delete service credentials')
    }
}
