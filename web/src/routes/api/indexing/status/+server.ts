import { db } from '$lib/server/db/index.js'
import { documents } from '$lib/server/db/schema.js'
import { sql, type SQL } from 'drizzle-orm'
import { error } from '@sveltejs/kit'
import type { RequestHandler } from './$types.js'
import postgres from 'postgres'
import { constructDatabaseUrl } from '$lib/server/config.js'
import { logger } from '$lib/server/logger.js'

type SourceId = string

// Cache for document counts per source (refreshed every 30 seconds)
const documentCountCache = new Map<
    string,
    { counts: Record<SourceId, number>; timestamp: number }
>()
const DOCUMENT_COUNT_CACHE_TTL = 30000 // 30 seconds

async function getDocumentCounts(
    sourceFilter: SQL,
    cacheKey: string,
): Promise<Record<SourceId, number>> {
    const now = Date.now()
    const cached = documentCountCache.get(cacheKey)
    if (cached && now - cached.timestamp < DOCUMENT_COUNT_CACHE_TTL) {
        return cached.counts
    }

    try {
        const counts = await db.execute(sql`
            SELECT
                d.source_id AS "sourceId",
                COUNT(*)::int AS count
            FROM ${documents} d
            JOIN sources s ON s.id = d.source_id
            WHERE s.is_deleted = false
            ${sourceFilter}
            GROUP BY d.source_id
        `)

        const countMap: Record<SourceId, number> = {}
        for (const row of counts) {
            countMap[row.sourceId as SourceId] = Number(row.count)
        }

        documentCountCache.set(cacheKey, { counts: countMap, timestamp: now })
        return countMap
    } catch (err) {
        logger.error('Error fetching document counts:', err)
        return cached?.counts ?? {}
    }
}

export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const isAdmin = locals.user.role === 'admin'
    const userId = locals.user.id
    const requestedScope = url.searchParams.get('scope')
    if (requestedScope && !['org', 'user', 'all'].includes(requestedScope)) {
        throw error(400, 'scope must be "org", "user", or "all"')
    }

    const statusScope = requestedScope ?? (isAdmin ? 'all' : 'user')
    if ((statusScope === 'org' || statusScope === 'all') && !isAdmin) {
        throw error(403, 'Forbidden')
    }

    const sourceFilter =
        statusScope === 'org'
            ? sql`AND s.scope = 'org'`
            : statusScope === 'user'
              ? sql`AND s.scope = 'user' AND s.created_by = ${userId}`
              : sql``
    const documentCountCacheKey = statusScope === 'user' ? `user:${userId}` : statusScope

    const encoder = new TextEncoder()
    let isClosed = false
    let listenSql: postgres.Sql | null = null
    let pollingInterval: ReturnType<typeof setInterval> | null = null

    const cleanup = async () => {
        isClosed = true
        if (listenSql) {
            try {
                await listenSql.end()
            } catch (error) {
                logger.error('Error closing listen connection:', error)
            }
            listenSql = null
        }
        if (pollingInterval) {
            clearInterval(pollingInterval)
            pollingInterval = null
        }
    }

    const stream = new ReadableStream({
        async start(controller) {
            // Function to send data to client
            const sendData = (data: unknown) => {
                if (isClosed) return

                try {
                    const message = `data: ${JSON.stringify(data)}\n\n`
                    controller.enqueue(encoder.encode(message))
                } catch (error) {
                    logger.error('Error sending SSE data:', error)
                    isClosed = true
                }
            }

            // Function to fetch and send status updates
            let isFetching = false
            const fetchStatus = async () => {
                if (isClosed || isFetching) return

                isFetching = true
                try {
                    // Get the latest sync run for each connected source
                    const result = await db.execute(sql`
                        SELECT DISTINCT ON (s.id)
                            sr.id,
                            s.id AS "sourceId",
                            s.name AS "sourceName",
                            s.source_type AS "sourceType",
                            sr.sync_type AS "syncType",
                            sr.status,
                            sr.documents_scanned AS "documentsScanned",
                            sr.documents_processed AS "documentsProcessed",
                            sr.documents_updated AS "documentsUpdated",
                            sr.started_at AS "startedAt",
                            sr.completed_at AS "completedAt",
                            sr.error_message AS "errorMessage"
                        FROM sources s
                        LEFT JOIN sync_runs sr ON sr.source_id = s.id
                        WHERE s.is_deleted = false
                        ${sourceFilter}
                        ORDER BY s.id, sr.started_at DESC NULLS LAST
                    `)
                    const latestSyncRuns = [...result]

                    // Get cached document counts per source
                    const documentCounts = await getDocumentCounts(
                        sourceFilter,
                        documentCountCacheKey,
                    )

                    const statusData = {
                        timestamp: Date.now(),
                        overall: {
                            latestSyncRuns,
                            documentCounts,
                        },
                    }

                    sendData(statusData)
                } catch (error) {
                    logger.error('Error fetching indexing status:', error)
                    if (!isClosed) {
                        sendData({ error: 'Failed to fetch status', timestamp: Date.now() })
                    }
                } finally {
                    isFetching = false
                }
            }

            // Setup PostgreSQL LISTEN/NOTIFY for real-time updates with throttling
            let lastUpdate = 0
            const MIN_UPDATE_INTERVAL = 1000 // Minimum 1 second between updates

            const throttledFetchStatus = async () => {
                const now = Date.now()
                if (now - lastUpdate < MIN_UPDATE_INTERVAL) {
                    return
                }
                lastUpdate = now
                await fetchStatus()
            }

            const setupNotifications = async () => {
                try {
                    listenSql = postgres(constructDatabaseUrl(), {
                        max: 1,
                        idle_timeout: 0,
                    })

                    // Listen for sync_runs updates
                    await listenSql.listen('sync_run_update', async () => {
                        logger.debug('Received sync_run_update notification')
                        if (!isClosed) {
                            // Fetch and send updated status when we receive notification (throttled)
                            await throttledFetchStatus()
                        }
                    })

                    logger.info('PostgreSQL LISTEN/NOTIFY setup successful')
                } catch (error) {
                    logger.error('Error setting up PostgreSQL notifications:', error)
                    // Fall back to polling if LISTEN/NOTIFY fails
                    pollingInterval = setInterval(() => {
                        if (!isClosed) {
                            throttledFetchStatus()
                        }
                    }, 10000)
                }
            }

            // Send initial data
            await fetchStatus()

            // Setup real-time notifications
            await setupNotifications()
        },

        cancel() {
            cleanup()
        },
    })

    return new Response(stream, {
        headers: {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache',
            Connection: 'keep-alive',
            'Access-Control-Allow-Origin': '*',
            'Access-Control-Allow-Headers': 'Cache-Control',
        },
    })
}
