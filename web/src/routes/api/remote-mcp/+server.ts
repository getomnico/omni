import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { and, eq, inArray, isNull, ne, sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { db } from '$lib/server/db'
import { serviceCredentials, sources, type Source } from '$lib/server/db/schema'
import { encryptConfig } from '$lib/server/crypto/encryption'
import { AuthType, IntegrationType, ServiceProvider } from '$lib/types'
import {
    probeRemoteMcpServer,
    remoteMcpConfigFromInput,
    validateRemoteMcpSlug,
    type RemoteMcpConfig,
} from '$lib/server/mcp/client'

type RemoteMcpSourceResponse = {
    id: string
    name: string
    sourceType: string
    integrationType: string
    scope: string
    config: unknown
    isActive: boolean
    isDeleted: boolean
    syncIntervalSeconds: number | null
    createdAt: Date
    updatedAt: Date
    authType: string | null
    isConnected: boolean
}

function remoteMcpAuthType(source: Source): string | null {
    const config = source.config as Partial<RemoteMcpConfig>
    return config.auth_type ?? null
}

function sanitizeRemoteMcpSource(
    source: Source,
    hasOrgCredential: boolean,
): RemoteMcpSourceResponse {
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

async function lockSourceSlug(tx: any, sourceType: string): Promise<void> {
    await tx.execute(sql`SELECT pg_advisory_xact_lock(hashtext(${`source_slug:${sourceType}`}))`)
}

async function assertNoActiveSlugCollision(
    tx: any,
    sourceType: string,
    excludingSourceId?: string,
): Promise<void> {
    const rows = await tx
        .select({ id: sources.id, integrationType: sources.integrationType })
        .from(sources)
        .where(
            and(
                eq(sources.sourceType, sourceType),
                eq(sources.isActive, true),
                eq(sources.isDeleted, false),
                excludingSourceId ? ne(sources.id, excludingSourceId) : undefined,
            ),
        )
        .limit(1)

    if (rows.length > 0) {
        const conflict = rows[0]
        const kind =
            conflict.integrationType === IntegrationType.REMOTE_MCP
                ? 'remote MCP'
                : 'native connector'
        throw error(409, `An active ${kind} source already uses sourceType ${sourceType}`)
    }
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

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    if (locals.user.role !== 'admin') throw error(403, 'Admin access required')

    const remoteSources = await db
        .select()
        .from(sources)
        .where(
            and(
                eq(sources.integrationType, IntegrationType.REMOTE_MCP),
                eq(sources.isDeleted, false),
            ),
        )

    const sourceIds = remoteSources.map((source) => source.id)
    const orgCredentials =
        sourceIds.length > 0
            ? await db
                  .select({ sourceId: serviceCredentials.sourceId })
                  .from(serviceCredentials)
                  .where(
                      and(
                          inArray(serviceCredentials.sourceId, sourceIds),
                          eq(serviceCredentials.provider, ServiceProvider.REMOTE_MCP),
                          isNull(serviceCredentials.userId),
                      ),
                  )
            : []
    const credentialSourceIds = new Set(orgCredentials.map((credential) => credential.sourceId))

    return json(
        remoteSources.map((source) =>
            sanitizeRemoteMcpSource(source, credentialSourceIds.has(source.id)),
        ),
    )
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) throw error(401, 'Unauthorized')
    const user = locals.user
    if (user.role !== 'admin') throw error(403, 'Admin access required')

    const body = await request.json()
    const name = typeof body.name === 'string' ? body.name.trim() : ''
    const sourceType = validateRemoteMcpSlug(String(body.sourceType ?? body.source_type ?? ''))
    const config = remoteMcpConfigFromInput({
        endpointUrl: String(body.endpointUrl ?? body.endpoint_url ?? ''),
        authType: body.authType ?? body.auth_type ?? null,
        writeToolsEnabled: body.writeToolsEnabled ?? body.write_tools_enabled,
    })
    const rawBearerToken = body.bearerToken ?? body.bearer_token
    const bearerToken = typeof rawBearerToken === 'string' ? rawBearerToken : null

    if (!name) throw error(400, 'name is required')
    if (config.auth_type === AuthType.BEARER_TOKEN && !bearerToken) {
        throw error(400, 'bearerToken is required for bearer auth')
    }

    const probe = await probeRemoteMcpServer({
        endpointUrl: config.endpoint_url,
        authType: config.auth_type,
        bearerToken,
    })
    if (!probe.ok) throw error(400, probe.error ?? 'Remote MCP probe failed')

    const [created] = await db.transaction(async (tx) => {
        await lockSourceSlug(tx, sourceType)
        await assertNoActiveSlugCollision(tx, sourceType)

        const inserted = await tx
            .insert(sources)
            .values({
                id: ulid(),
                name,
                sourceType,
                integrationType: IntegrationType.REMOTE_MCP,
                scope: 'org',
                config,
                createdBy: user.id,
                isActive: config.auth_type !== AuthType.OAUTH,
                syncIntervalSeconds: null,
            })
            .returning()

        if (config.auth_type === AuthType.BEARER_TOKEN && bearerToken) {
            await upsertRemoteMcpBearerCredential(tx, inserted[0].id, bearerToken)
        }

        return inserted
    })

    return json(
        { ...sanitizeRemoteMcpSource(created, config.auth_type === AuthType.BEARER_TOKEN), probe },
        { status: 201 },
    )
}
