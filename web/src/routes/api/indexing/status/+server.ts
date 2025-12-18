import { db } from '$lib/server/db/index.js'
import { syncRuns, sources, documents } from '$lib/server/db/schema.js'
import { eq, desc, count, sql } from 'drizzle-orm'
import type { RequestHandler } from './$types.js'
import postgres from 'postgres'
import { constructDatabaseUrl } from '$lib/server/config.js'
import { logger } from '$lib/server/logger.js'

type SourceId = string

// Cache for document counts per source (refreshed every 30 seconds)
let documentCountCache: Record<SourceId, number> = {}
let documentCountCacheTime = 0
const DOCUMENT_COUNT_CACHE_TTL = 30000 // 30 seconds

async function getDocumentCounts(): Promise<Record<SourceId, number>> {
    const now = Date.now()
    if (now - documentCountCacheTime < DOCUMENT_COUNT_CACHE_TTL) {
        return documentCountCache
    }

    try {
        const counts = await db
            .select({
                sourceId: documents.sourceId,
                count: count(),
            })
            .from(documents)
            .groupBy(documents.sourceId)

        const countMap: Record<SourceId, number> = {}
        for (const row of counts) {
            countMap[row.sourceId] = row.count
        }

        documentCountCache = countMap
        documentCountCacheTime = now
        return countMap
    } catch (error) {
        logger.error('Error fetching document counts:', error)
        return documentCountCache // Return stale cache on error
    }
}

export const GET: RequestHandler = async ({ url }) => {
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
            const sendData = (data: any) => {
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
                        ORDER BY s.id, sr.started_at DESC NULLS LAST
                    `)
                    const latestSyncRuns = [...result]

                    // Get cached document counts per source
                    const documentCounts = await getDocumentCounts()

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
                    // Fall back to polling if LISTEN/NOTIFY fails (every 10 seconds to avoid spam)
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
