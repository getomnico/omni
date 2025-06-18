import { db } from '$lib/server/db/index.js'
import { connectorEventsQueue, sources } from '$lib/server/db/schema.js'
import { sql, eq, desc } from 'drizzle-orm'
import type { RequestHandler } from './$types.js'

export const GET: RequestHandler = async ({ url }) => {
    const stream = new ReadableStream({
        start(controller) {
            const encoder = new TextEncoder()
            let isClosed = false

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
                    // Get overall status counts
                    const statusCounts = await db
                        .select({
                            status: connectorEventsQueue.status,
                            count: sql<number>`count(*)::int`,
                        })
                        .from(connectorEventsQueue)
                        .groupBy(connectorEventsQueue.status)

                    // Get per-source status counts
                    const sourceStatus = await db
                        .select({
                            sourceId: connectorEventsQueue.sourceId,
                            sourceName: sources.name,
                            sourceType: sources.sourceType,
                            status: connectorEventsQueue.status,
                            count: sql<number>`count(*)::int`,
                        })
                        .from(connectorEventsQueue)
                        .leftJoin(sources, eq(connectorEventsQueue.sourceId, sources.id))
                        .groupBy(
                            connectorEventsQueue.sourceId,
                            sources.name,
                            sources.sourceType,
                            connectorEventsQueue.status,
                        )

                    // Get recent activity (last 10 processed events)
                    const recentActivity = await db
                        .select({
                            id: connectorEventsQueue.id,
                            sourceId: connectorEventsQueue.sourceId,
                            sourceName: sources.name,
                            eventType: connectorEventsQueue.eventType,
                            status: connectorEventsQueue.status,
                            processedAt: connectorEventsQueue.processedAt,
                            errorMessage: connectorEventsQueue.errorMessage,
                        })
                        .from(connectorEventsQueue)
                        .leftJoin(sources, eq(connectorEventsQueue.sourceId, sources.id))
                        .where(
                            sql`${connectorEventsQueue.status} IN ('completed', 'failed', 'processing')`,
                        )
                        .orderBy(desc(connectorEventsQueue.processedAt))
                        .limit(10)

                    // Transform data for easier consumption
                    const statusMap = statusCounts.reduce(
                        (acc, item) => {
                            acc[item.status] = item.count
                            return acc
                        },
                        {} as Record<string, number>,
                    )

                    const sourceStatusMap = sourceStatus.reduce(
                        (acc, item) => {
                            if (!acc[item.sourceId]) {
                                acc[item.sourceId] = {
                                    pending: 0,
                                    processing: 0,
                                    completed: 0,
                                    failed: 0,
                                }
                            }
                            acc[item.sourceId][item.status] = item.count
                            return acc
                        },
                        {} as Record<string, any>,
                    )

                    const statusData = {
                        timestamp: Date.now(),
                        overall: {
                            pending: statusMap.pending || 0,
                            processing: statusMap.processing || 0,
                            completed: statusMap.completed || 0,
                            failed: statusMap.failed || 0,
                            dead_letter: statusMap.dead_letter || 0,
                        },
                        sources: sourceStatusMap,
                        recentActivity,
                    }

                    sendData(statusData)
                } catch (error) {
                    console.error('Error fetching indexing status:', error)
                    if (!isClosed) {
                        sendData({ error: 'Failed to fetch status' })
                    }
                }
            }

            // Send initial data
            fetchStatus()

            // Send updates every 3 seconds
            const interval = setInterval(fetchStatus, 3000)

            // Cleanup on connection close
            return () => {
                isClosed = true
                clearInterval(interval)
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
