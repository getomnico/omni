/**
 * Runtime test for OTel log export via the production SDK factory.
 *
 * Uses vitest's `pool: forks` — each test file runs in its own process,
 * so we can control import ordering.  We import the SDK factory *before*
 * Pino (matching production `--import` bootstrap), then emit a log
 * within an active span and verify /v1/logs receipt.
 *
 * This test MUST be run by itself (it seals a local HTTP collector stub)
 * and is excluded from the general test suite by the `.test.ts` naming.
 *
 * The test does NOT call @opentelemetry/api-logs directly — it relies
 * entirely on the production Pino hook (otel-log-hook.mjs) for log
 * emission.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { createServer, type Server } from 'http'
import { trace } from '@opentelemetry/api'
import { Writable } from 'stream'

// ---------------------------------------------------------------------------
// Collector stub
// ---------------------------------------------------------------------------

interface LogsRequest {
    body: string
    parsed: Record<string, unknown>
}

let collectorServer: Server | null = null
let collectorPort = 0
const receivedLogs: LogsRequest[] = []

function startCollector(): Promise<number> {
    return new Promise((resolve, reject) => {
        const server = createServer((req, res) => {
            let body = ''
            req.on('data', (chunk: Buffer) => {
                body += chunk.toString()
            })
            req.on('end', () => {
                if (req.url === '/v1/logs' && req.method === 'POST') {
                    try {
                        const parsed = JSON.parse(body)
                        receivedLogs.push({ body, parsed })
                    } catch {
                        receivedLogs.push({ body, parsed: { _raw: body } })
                    }
                }
                res.writeHead(200, { 'Content-Type': 'application/json' })
                res.end('{}')
            })
        })

        server.listen(0, '127.0.0.1', () => {
            const addr = server.address()
            if (addr && typeof addr === 'object') {
                collectorServer = server
                resolve(addr.port)
            } else {
                reject(new Error('Failed to get collector port'))
            }
        })

        server.on('error', reject)
    })
}

function stopCollector(): Promise<void> {
    return new Promise((resolve) => {
        if (collectorServer) {
            collectorServer.close(() => resolve())
            collectorServer = null
        } else {
            resolve()
        }
    })
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

describe('OTel log export runtime', () => {
    beforeAll(async () => {
        collectorPort = await startCollector()
    }, 15000)

    afterAll(async () => {
        await stopCollector()
    }, 15000)

    it('emits Pino log within an active span and receives /v1/logs with trace/span IDs, no duplicate', async () => {
        receivedLogs.length = 0

        const otlpEndpoint = `http://127.0.0.1:${collectorPort}`
        process.env.OTEL_EXPORTER_OTLP_ENDPOINT = otlpEndpoint
        process.env.OTEL_DEPLOYMENT_ID = 'test-deployment'
        process.env.OTEL_DEPLOYMENT_ENVIRONMENT = 'test'
        process.env.SERVICE_VERSION = '0.0.0-test'

        // Import SDK factory FIRST — before any Pino instrumentation.
        const { createSdk } = await import('../../otel-factory.mjs')
        const { sdk, logRecordProcessors } = createSdk({ otlpEndpoint })
        sdk.start()

        // Wait briefly for SDK initialization.
        await new Promise((r) => setTimeout(r, 100))

        // Import Pino (after SDK start — OTel auto-instrumentation
        // will patch Pino when it's first loaded).
        const { default: pino } = await import('pino')

        // Import the production hook and mixin to build an exact-production logger.
        const { createPinoOtelHook } = await import('./otel-log-hook.mjs')
        const productionHook = createPinoOtelHook()

        // ------------------------------------------------------------------
        // Stdout capture stream — records serialized JSON lines for mixin
        // correlation assertion.
        // ------------------------------------------------------------------
        const stdoutChunks: Buffer[] = []
        const captureStream = new Writable({
            write(chunk: Buffer, _encoding, callback) {
                stdoutChunks.push(chunk)
                callback()
            },
        })

        // Create a Pino logger with the production hook/mixin.
        const pinoLogger = pino(
            {
                level: 'info',
                name: 'test-logger',
                timestamp: pino.stdTimeFunctions.isoTime,
                mixin(_context: object, _level: number) {
                    const span = trace.getActiveSpan()
                    if (!span) return {}
                    const spanCtx = span.spanContext()
                    return {
                        trace_id: spanCtx.traceId,
                        span_id: spanCtx.spanId,
                    }
                },
                hooks: {
                    logMethod: productionHook,
                },
            },
            captureStream,
        )

        // Create a tracer and start an active span so log records
        // carry native trace_id / span_id.
        const tracer = trace.getTracer('test-tracer')

        await tracer.startActiveSpan('test-span', async (span) => {
            // Emit ONE Pino log — the hook will export it to OTel.
            pinoLogger.info({ operation: 'runtime-test' }, 'test log message from runtime')
            span.end()
        })

        // Force-flush log processors
        if (logRecordProcessors) {
            for (const p of logRecordProcessors) {
                await p.forceFlush()
            }
        }

        await sdk.shutdown()

        // ------------------------------------------------------------------
        // Assertions
        // ------------------------------------------------------------------

        // 1. Verify stdout correlation (mixin injected trace_id/span_id)
        const stdoutLine = stdoutChunks.map((b) => b.toString()).join('')
        const stdoutParsed = JSON.parse(stdoutLine.trim())
        expect(stdoutParsed).toHaveProperty('trace_id')
        expect(stdoutParsed).toHaveProperty('span_id')
        expect(stdoutParsed.trace_id).toBeTruthy()
        expect(stdoutParsed.span_id).toBeTruthy()
        // Verify they are non-zero
        expect(stdoutParsed.trace_id).not.toMatch(/^0+$/)
        expect(stdoutParsed.span_id).not.toMatch(/^0+$/)
        // Verify the message is present in stdout
        expect(stdoutParsed.msg).toBe('test log message from runtime')

        // 2. Verify at least one /v1/logs request was received
        expect(receivedLogs.length).toBeGreaterThanOrEqual(1)

        // Find the resourceLogs entry that contains our log record
        const entry = receivedLogs.find(
            (r) => r.parsed && Array.isArray((r.parsed as any).resourceLogs),
        )
        expect(entry).toBeDefined()
        if (!entry) return

        const resourceLogs = (entry.parsed as any).resourceLogs as any[]
        const logRecords: any[] = []

        for (const rl of resourceLogs) {
            const scopeLogs = rl.scopeLogs
            if (Array.isArray(scopeLogs)) {
                for (const sl of scopeLogs) {
                    if (Array.isArray(sl.logRecords)) {
                        logRecords.push(...sl.logRecords)
                    }
                }
            }
        }

        // 3. Assert exactly ONE log record with matching body and operation attribute
        const matchingRecords = logRecords.filter(
            (lr: any) =>
                lr.body?.stringValue === 'test log message from runtime' &&
                lr.attributes?.some(
                    (attr: any) =>
                        attr.key === 'operation' && attr.value?.stringValue === 'runtime-test',
                ),
        )
        expect(matchingRecords).toHaveLength(1)

        const logRecord = matchingRecords[0]

        // 4. Verify trace_id / span_id are present and non-empty
        expect(logRecord.traceId).toBeDefined()
        expect(logRecord.spanId).toBeDefined()
        const traceIdHex = Buffer.from(logRecord.traceId as string, 'base64').toString('hex')
        const spanIdHex = Buffer.from(logRecord.spanId as string, 'base64').toString('hex')
        expect(traceIdHex).not.toMatch(/^0+$/)
        expect(spanIdHex).not.toMatch(/^0+$/)

        // 5. Verify severity mapping
        expect(logRecord.severityNumber).toBe(9) // INFO
        expect(logRecord.severityText).toBe('INFO')

        // 6. Verify no duplicate /v1/logs with the same log record
        // A duplicate would mean either:
        //   a) Multiple matching records in one batch, or
        //   b) Multiple /v1/logs requests each containing the record
        // We already asserted exactly 1 matching record above.
        // Also verify we didn't get multiple matching requests.
        const matchingRequests = receivedLogs.filter((r) => {
            if (!r.parsed || !Array.isArray((r.parsed as any).resourceLogs)) return false
            const rls = (r.parsed as any).resourceLogs as any[]
            for (const rl of rls) {
                const sls = rl.scopeLogs
                if (!Array.isArray(sls)) continue
                for (const sl of sls) {
                    const lrs = sl.logRecords
                    if (!Array.isArray(lrs)) continue
                    for (const lr of lrs) {
                        if (
                            lr.body?.stringValue === 'test log message from runtime' &&
                            lr.attributes?.some(
                                (attr: any) =>
                                    attr.key === 'operation' &&
                                    attr.value?.stringValue === 'runtime-test',
                            )
                        ) {
                            return true
                        }
                    }
                }
            }
            return false
        })
        expect(matchingRequests).toHaveLength(1)
    })
})
