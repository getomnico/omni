import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, serviceCredentials, syncRuns } from '$lib/server/db/schema'
import { and, eq, inArray, sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { logger } from '$lib/server/logger'
import { IntegrationType, SourceType, DEFAULT_SYNC_INTERVAL_SECONDS } from '$lib/types'
import { getSourcesByType } from '$lib/server/db/sources'
import { isSyncDisabledConfig } from '$lib/utils/sources'

function validateAllowedActionOrigins(value: unknown): string[] | undefined {
    if (value === undefined) return undefined
    if (!Array.isArray(value) || value.some((origin) => origin !== 'native' && origin !== 'mcp')) {
        throw error(
            400,
            "allowed_action_origins must be an array containing only 'native' or 'mcp'",
        )
    }
    return value
}

function validateSalesforceUrl(value: unknown, kind: 'login' | 'instance'): string {
    if (typeof value !== 'string' || !value.trim()) {
        throw error(400, `Salesforce ${kind} URL is required`)
    }
    const raw = value.trim()
    const normalized = /^https:\/\//i.test(raw) ? raw : `https://${raw}`
    let parsed: URL
    try {
        parsed = new URL(normalized)
    } catch {
        throw error(400, `Invalid Salesforce ${kind} URL`)
    }
    const host = parsed.hostname.toLowerCase().replace(/\.$/, '')
    const loginHost =
        host === 'login.salesforce.com' ||
        host === 'test.salesforce.com' ||
        host.endsWith('.my.salesforce.com')
    const instanceHost = host.endsWith('.salesforce.com') || host.endsWith('.force.com')
    if (
        parsed.protocol !== 'https:' ||
        parsed.username ||
        parsed.password ||
        parsed.port ||
        parsed.search ||
        parsed.hash ||
        (parsed.pathname !== '/' && parsed.pathname !== '') ||
        (kind === 'login' ? !loginHost : !instanceHost)
    ) {
        throw error(400, `Invalid Salesforce ${kind} URL`)
    }
    return parsed.origin
}

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const allSources = await db.query.sources.findMany()
    logger.debug(`/api/sources: found ${allSources.length} sources.`)

    // Get service credentials for all sources
    const sourceIds = allSources.map((s) => s.id)
    const credentials =
        sourceIds.length > 0
            ? await db.query.serviceCredentials.findMany({
                  where: inArray(serviceCredentials.sourceId, sourceIds),
              })
            : []

    // Get latest sync run for each source
    const latestSyncRuns =
        sourceIds.length > 0
            ? await db
                  .select()
                  .from(syncRuns)
                  .where(
                      sql`${syncRuns.id} IN (
                          SELECT DISTINCT ON (source_id) id
                          FROM sync_runs
                          WHERE source_id IN ${sourceIds}
                          ORDER BY source_id, started_at DESC
                      )`,
                  )
            : []
    logger.debug(`/api/sources: found ${latestSyncRuns.length} latest sync runs.`)

    const syncRunMap = new Map(latestSyncRuns.map((r) => [r.sourceId, r]))

    // Create a map of source ID to whether it has credentials
    const credentialsMap = new Map(credentials.map((c) => [c.sourceId, true]))

    const sanitizedSources = allSources.map((source) => {
        const latestSync = syncRunMap.get(source.id)
        let sourceConfig = source.config
        if (locals.user?.role !== 'admin' && source.sourceType === SourceType.DARWINBOX) {
            const config = structuredClone((source.config ?? {}) as Record<string, unknown>)
            delete config.authorization
            delete config.employee_scope
            sourceConfig = config
        }
        const isSyncDisabled = isSyncDisabledConfig(source.config)
        return {
            id: source.id,
            name: source.name,
            sourceType: source.sourceType,
            integrationType: source.integrationType,
            scope: source.scope,
            config: sourceConfig,
            syncStatus: isSyncDisabled ? null : (latestSync?.status ?? null),
            isActive: source.isActive,
            lastSyncAt: isSyncDisabled ? null : (latestSync?.completedAt ?? null),
            syncError: isSyncDisabled ? null : (latestSync?.errorMessage ?? null),
            createdAt: source.createdAt,
            updatedAt: source.updatedAt,
            isConnected: credentialsMap.has(source.id),
        }
    })

    return json(sanitizedSources)
}

export const POST: RequestHandler = async ({ request, locals }) => {
    const user = locals.user
    if (!user) {
        throw error(401, 'Unauthorized')
    }

    const body = await request.json()
    const { name, sourceType, config, isActive } = body
    // Scope: 'org' (admin-set-up, shared across users) or 'user' (personal). Defaults to
    // 'user' to preserve existing behavior of the per-user OAuth connect flow that doesn't
    // pass a scope.
    const scope: 'org' | 'user' = body.scope === 'org' ? 'org' : 'user'

    let sourceConfig: Record<string, unknown> =
        config && typeof config === 'object' && !Array.isArray(config) ? { ...config } : {}
    if (Object.prototype.hasOwnProperty.call(sourceConfig, 'allowed_action_origins')) {
        sourceConfig.allowed_action_origins = validateAllowedActionOrigins(
            sourceConfig.allowed_action_origins,
        )
    }
    const isSyncDisabled = isSyncDisabledConfig(sourceConfig)
    const requestedIsActive = isActive ?? false
    if (sourceType === SourceType.SALESFORCE) {
        sourceConfig.login_url = validateSalesforceUrl(sourceConfig.login_url, 'login')
        if (isSyncDisabled) {
            delete sourceConfig.instance_url
        } else {
            sourceConfig.instance_url = validateSalesforceUrl(sourceConfig.instance_url, 'instance')
        }
    }

    if (!name || !sourceType) {
        throw error(400, 'Name and sourceType are required')
    }

    // SA-direct is Drive-only: reject crafted Gmail/Chat source creation.
    const configAuthMode =
        config && typeof config === 'object' && !Array.isArray(config)
            ? (config as Record<string, unknown>).auth_mode
            : undefined
    if (configAuthMode === 'service_account_direct' && sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, 'service_account_direct is supported only for Google Drive sources')
    }

    if (
        body.integrationType === IntegrationType.REMOTE_MCP ||
        body.integration_type === IntegrationType.REMOTE_MCP
    ) {
        throw error(400, 'Remote MCP sources must be created through /api/remote-mcp')
    }

    if (scope === 'org' && user.role !== 'admin') {
        throw error(403, 'Only admins can create org-wide sources')
    }
    const sourcesOfType = await getSourcesByType(sourceType)

    if (scope === 'user') {
        // OAuth-based connectors should only have one personal source per user. Other
        // connectors (e.g. web) can have multiple instances.
        // TODO: Consider adding other OAuth connectors (e.g. Outlook, Slack) as they support user-level OAuth.
        const uniqueSourceTypes: string[] = [SourceType.GOOGLE_DRIVE, SourceType.GMAIL]
        if (uniqueSourceTypes.includes(sourceType)) {
            const existingForUser = sourcesOfType.find(
                (s) => s.scope === 'user' && s.createdBy === user.id,
            )
            if (existingForUser) {
                throw error(409, `A ${sourceType} source already exists`)
            }
        }
    }

    const [newSource] = await db.transaction(async (tx) => {
        await tx.execute(
            sql`SELECT pg_advisory_xact_lock(hashtext(${`source_slug:${sourceType}`}))`,
        )

        if (requestedIsActive) {
            const remoteConflicts = await tx
                .select({ id: sources.id })
                .from(sources)
                .where(
                    and(
                        eq(sources.sourceType, sourceType),
                        eq(sources.integrationType, IntegrationType.REMOTE_MCP),
                        eq(sources.isActive, true),
                        eq(sources.isDeleted, false),
                    ),
                )
                .limit(1)
            if (remoteConflicts.length > 0) {
                throw error(
                    409,
                    `An active remote MCP source already uses sourceType ${sourceType}`,
                )
            }
        }

        return await tx
            .insert(sources)
            .values({
                id: ulid(),
                name,
                sourceType,
                integrationType: IntegrationType.CONNECTOR,
                scope,
                config: sourceConfig,
                createdBy: user.id,
                // A disabled sync interval prevents scheduling for any connector.
                isActive: requestedIsActive,
                syncIntervalSeconds: isSyncDisabled
                    ? null
                    : DEFAULT_SYNC_INTERVAL_SECONDS[sourceType as SourceType],
            })
            .returning()
    })

    return json({
        id: newSource.id,
        name: newSource.name,
        sourceType: newSource.sourceType,
        integrationType: newSource.integrationType,
        scope: newSource.scope,
        config: newSource.config,
        syncStatus: null,
        isActive: newSource.isActive,
        lastSyncAt: null,
        syncError: null,
        createdAt: newSource.createdAt,
        updatedAt: newSource.updatedAt,
        isConnected: false,
    })
}
