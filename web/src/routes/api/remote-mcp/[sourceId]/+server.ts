import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { and, eq, isNull, sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { getConfig } from '$lib/server/config'
import { db } from '$lib/server/db'
import { serviceCredentials, sources, type Source } from '$lib/server/db/schema'
import { decryptConfig, encryptConfig } from '$lib/server/crypto/encryption'
import { AuthType, IntegrationType, ServiceProvider } from '$lib/types'
import {
    probeRemoteMcpServer,
    remoteMcpConfigFromInput,
    type RemoteMcpConfig,
} from '$lib/server/mcp/client'

function remoteMcpAuthType(source: Source): string | null {
    const config = source.config as Partial<RemoteMcpConfig>
    return config.auth_type ?? null
}

function sanitizeRemoteMcpSource(source: Source, hasOrgCredential: boolean) {
    const authType = remoteMcpAuthType(source)
    return {
        id: source.id,
        name: source.name,
        sourceType: source.sourceType,
        integrationType: source.integrationType,
        scope: source.scope,
        config: source.config,
        isActive: source.isActive,
        isDeleted: source.isDeleted,
        syncIntervalSeconds: source.syncIntervalSeconds,
        createdAt: source.createdAt,
        updatedAt: source.updatedAt,
        authType,
        isConnected: authType === AuthType.BEARER_TOKEN ? hasOrgCredential : source.isActive,
    }
}

async function loadRemoteMcpSource(sourceId: string): Promise<Source> {
    const source = await db.query.sources.findFirst({ where: eq(sources.id, sourceId) })
    if (!source || source.isDeleted || source.integrationType !== IntegrationType.REMOTE_MCP) {
        throw error(404, 'Remote MCP source not found')
    }
    return source
}

async function getOrgRemoteMcpCredential(sourceId: string) {
    return await db.query.serviceCredentials.findFirst({
        where: and(
            eq(serviceCredentials.sourceId, sourceId),
            eq(serviceCredentials.provider, ServiceProvider.REMOTE_MCP),
            isNull(serviceCredentials.userId),
        ),
    })
}

function bearerTokenFromCredential(
    credential: { credentials: unknown } | undefined,
): string | null {
    if (!credential) return null
    const decrypted = decryptConfig(credential.credentials)
    const token = decrypted.token
    return typeof token === 'string' ? token : null
}

export function remoteMcpPutTransition(
    existingIsActive: boolean,
    previousConfig: Partial<RemoteMcpConfig>,
    nextConfig: RemoteMcpConfig,
): {
    shouldBeActive: boolean
    shouldDeleteCredentials: boolean
    oauthBootstrapRequired: boolean
} {
    const previousAuthType = previousConfig.auth_type ?? null
    const nextAuthType = nextConfig.auth_type ?? null
    const authTypeChanged = previousAuthType !== nextAuthType
    const endpointChanged = previousConfig.endpoint_url !== nextConfig.endpoint_url
    const oauthBootstrapRequired =
        nextAuthType === AuthType.OAUTH && (authTypeChanged || endpointChanged)

    return {
        shouldBeActive:
            nextAuthType === AuthType.OAUTH ? existingIsActive && !oauthBootstrapRequired : true,
        shouldDeleteCredentials:
            authTypeChanged ||
            (previousAuthType === AuthType.OAUTH &&
                nextAuthType === AuthType.OAUTH &&
                endpointChanged),
        oauthBootstrapRequired,
    }
}

async function pruneRemoteMcpCapabilities(sourceId: string, fetchFn: typeof fetch): Promise<void> {
    const searcherUrl = getConfig().services.searcherUrl
    await Promise.all(
        ['resource', 'prompt'].map((capabilityType) =>
            fetchFn(`${searcherUrl}/capabilities/sync`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    publisher_id: sourceId,
                    capability_type: capabilityType,
                    capabilities: [],
                }),
            }).catch(() => undefined),
        ),
    )
}

async function upsertRemoteMcpBearerCredential(
    tx: any,
    sourceId: string,
    bearerToken: string,
): Promise<void> {
    await tx
        .delete(serviceCredentials)
        .where(
            and(
                eq(serviceCredentials.sourceId, sourceId),
                eq(serviceCredentials.provider, ServiceProvider.REMOTE_MCP),
                isNull(serviceCredentials.userId),
            ),
        )

    await tx.insert(serviceCredentials).values({
        id: ulid(),
        sourceId,
        userId: null,
        provider: ServiceProvider.REMOTE_MCP,
        authType: AuthType.BEARER_TOKEN,
        principalEmail: null,
        credentials: encryptConfig({ token: bearerToken }),
        config: {},
    })
}

export const GET: RequestHandler = async ({ params, locals }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (locals.user.role !== 'admin') throw error(403, 'Admin access required')

    const source = await loadRemoteMcpSource(params.sourceId)
    const credential = await getOrgRemoteMcpCredential(source.id)
    return json(sanitizeRemoteMcpSource(source, Boolean(credential)))
}

export const PUT: RequestHandler = async ({ params, request, locals, fetch }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (locals.user.role !== 'admin') throw error(403, 'Admin access required')

    const existing = await loadRemoteMcpSource(params.sourceId)
    const existingConfig = existing.config as Partial<RemoteMcpConfig>
    const body = await request.json()

    const requestedSourceType = body.sourceType ?? body.source_type
    if (requestedSourceType !== undefined && requestedSourceType !== existing.sourceType) {
        throw error(400, 'Remote MCP sourceType is immutable')
    }

    const name = typeof body.name === 'string' ? body.name.trim() : existing.name
    if (!name) throw error(400, 'name is required')

    const config = remoteMcpConfigFromInput({
        endpointUrl: String(
            body.endpointUrl ?? body.endpoint_url ?? existingConfig.endpoint_url ?? '',
        ),
        authType: body.authType ?? body.auth_type ?? existingConfig.auth_type ?? null,
        writeToolsEnabled:
            body.writeToolsEnabled ??
            body.write_tools_enabled ??
            existingConfig.write_tools_enabled,
    })
    const rawBearerToken = body.bearerToken ?? body.bearer_token
    const providedBearerToken = typeof rawBearerToken === 'string' ? rawBearerToken : null
    const existingCredential = await getOrgRemoteMcpCredential(existing.id)
    const bearerToken =
        config.auth_type === AuthType.BEARER_TOKEN
            ? (providedBearerToken ?? bearerTokenFromCredential(existingCredential))
            : null

    if (config.auth_type === AuthType.BEARER_TOKEN && !bearerToken) {
        throw error(400, 'bearerToken is required for bearer auth')
    }

    const { shouldBeActive, shouldDeleteCredentials } = remoteMcpPutTransition(
        existing.isActive,
        existingConfig,
        config,
    )

    const probe = await probeRemoteMcpServer({
        endpointUrl: config.endpoint_url,
        authType: config.auth_type,
        bearerToken,
    })
    if (!probe.ok) throw error(400, probe.error ?? 'Remote MCP probe failed')

    const [updated] = await db.transaction(async (tx) => {
        await tx.execute(
            sql`SELECT pg_advisory_xact_lock(hashtext(${`source_slug:${existing.sourceType}`}))`,
        )

        if (shouldBeActive) {
            const conflicts = await tx
                .select({ id: sources.id })
                .from(sources)
                .where(
                    and(
                        eq(sources.sourceType, existing.sourceType),
                        eq(sources.isActive, true),
                        eq(sources.isDeleted, false),
                    ),
                )
                .limit(2)
            if (conflicts.some((row: { id: string }) => row.id !== existing.id)) {
                throw error(409, `An active source already uses sourceType ${existing.sourceType}`)
            }
        }

        const rows = await tx
            .update(sources)
            .set({ name, config, isActive: shouldBeActive, updatedAt: new Date() })
            .where(eq(sources.id, existing.id))
            .returning()

        if (config.auth_type === AuthType.BEARER_TOKEN && providedBearerToken) {
            await upsertRemoteMcpBearerCredential(tx, existing.id, providedBearerToken)
        } else if (shouldDeleteCredentials) {
            await tx
                .delete(serviceCredentials)
                .where(
                    and(
                        eq(serviceCredentials.sourceId, existing.id),
                        eq(serviceCredentials.provider, ServiceProvider.REMOTE_MCP),
                    ),
                )
        }

        return rows
    })

    if (existing.isActive && !updated.isActive) {
        await pruneRemoteMcpCapabilities(updated.id, fetch)
    }

    const credential = await getOrgRemoteMcpCredential(updated.id)
    return json({ ...sanitizeRemoteMcpSource(updated, Boolean(credential)), probe })
}

export const PATCH = PUT

export const DELETE: RequestHandler = async ({ params, locals, fetch }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (locals.user.role !== 'admin') throw error(403, 'Admin access required')

    const source = await loadRemoteMcpSource(params.sourceId)
    await db.transaction(async (tx) => {
        await tx.execute(
            sql`SELECT pg_advisory_xact_lock(hashtext(${`source_slug:${source.sourceType}`}))`,
        )
        await tx.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, source.id))
        await tx
            .update(sources)
            .set({ isActive: false, isDeleted: true, updatedAt: new Date() })
            .where(eq(sources.id, source.id))
    })

    await pruneRemoteMcpCapabilities(source.id, fetch)

    return json({ success: true })
}
