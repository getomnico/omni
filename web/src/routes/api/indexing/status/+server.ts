import { db } from '$lib/server/db/index.js'
import { syncRuns, sources, documents } from '$lib/server/db/schema.js'
import { sql, eq, desc } from 'drizzle-orm'
import type { RequestHandler } from './$types.js'
import postgres from 'postgres'
import { env } from '$env/dynamic/private'

export const GET: RequestHandler = async ({ url }) => {
    const stream = new ReadableStream({
        async start(controller) {
            const encoder = new TextEncoder()
            let isClosed = false
            let listenSql: any = null

            // Function to send data to client
            const sendData = (data: any) => {
                if (isClosed) return

                try {
                    const message = `data: ${JSON.stringify(data)}\n\n`
                    controller.enqueue(encoder.encode(message))
                } catch (error) {
                    console.error('Error sending SSE data:', error)
                    isClosed = true
                }
            }

            // Function to fetch and send status updates
            const fetchStatus = async () => {
                if (isClosed) return

                try {
                    // Get latest 10 sync runs (same as page load)
                    const latestSyncRuns = await db
                        .select({
                            id: syncRuns.id,
                            sourceId: syncRuns.sourceId,
                            sourceName: sources.name,
                            sourceType: sources.sourceType,
                            syncType: syncRuns.syncType,
                            status: syncRuns.status,
                            documentsProcessed: syncRuns.documentsProcessed,
                            documentsUpdated: syncRuns.documentsUpdated,
                            startedAt: syncRuns.startedAt,
                            completedAt: syncRuns.completedAt,
                            errorMessage: syncRuns.errorMessage,
                        })
                        .from(syncRuns)
                        .leftJoin(sources, eq(syncRuns.sourceId, sources.id))
                        .orderBy(desc(syncRuns.startedAt))
                        .limit(10)

                    // Get document counts by source
                    const documentsBySource = await db
                        .select({
                            sourceId: documents.sourceId,
                            count: sql<number>`COUNT(*)::int`,
                        })
                        .from(documents)
                        .groupBy(documents.sourceId)
                    const totalDocumentsIndexed = documentsBySource
                        .map((r) => r.count)
                        .reduce((a, v) => (a += v), 0)

                    const documentCountBySource = documentsBySource.reduce(
                        (acc, item) => {
                            acc[item.sourceId] = item.count
                            return acc
                        },
                        {} as Record<string, number>,
                    )

                    const statusData = {
                        timestamp: Date.now(),
                        overall: {
                            latestSyncRuns,
                            documentStats: {
                                totalDocumentsIndexed,
                                documentsBySource: documentCountBySource,
                            },
                        },
                    }

                    sendData(statusData)
                } catch (error) {
                    console.error('Error fetching indexing status:', error)
                    if (!isClosed) {
                        sendData({ error: 'Failed to fetch status' })
                    }
                }
            }

            // Setup PostgreSQL LISTEN/NOTIFY for real-time updates
            const setupNotifications = async () => {
                try {
                    listenSql = postgres(env.DATABASE_URL)

                    // Listen for sync_runs updates
                    await listenSql.listen('sync_run_update', async () => {
                        if (!isClosed) {
                            // Fetch and send updated status when we receive notification
                            await fetchStatus()
                        }
                    })
                } catch (error) {
                    console.error('Error setting up PostgreSQL notifications:', error)
                    // Fall back to polling if LISTEN/NOTIFY fails
                    const interval = setInterval(fetchStatus, 5000)
                    return () => clearInterval(interval)
                }
            }

            // Send initial data
            await fetchStatus()

            // Setup real-time notifications
            const cleanupNotifications = await setupNotifications()

            // Cleanup on connection close
            return () => {
                isClosed = true
                if (listenSql) {
                    listenSql.end().catch(console.error)
                }
                if (cleanupNotifications) {
                    cleanupNotifications()
                }
                try {
                    controller.close()
                } catch (error) {
                    // Controller might already be closed
                }
            }
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
