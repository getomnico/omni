/**
 * Telemetry API helpers.
 *
 * The OTel SDK is initialised by the preload bootstrap (`instrumentation.mjs`).
 * This module only exposes helper functions for manual instrumentation.
 *
 * Do NOT add SDK startup code here — that belongs in instrumentation.mjs.
 */

import { propagation, trace, context, type Span, metrics } from '@opentelemetry/api'

// ---------------------------------------------------------------------------
// Pure helpers (exported for testing)
// ---------------------------------------------------------------------------

/** Convert a duration from milliseconds to seconds. */
export function millisecondsToSeconds(ms: number): number {
    return ms / 1000
}

/** Build RED metric attributes with bounded values (method, route, statusCode). */
export function buildRedAttributes(
    method: string,
    route: string,
    statusCode: number,
): Record<string, string | number> {
    return {
        'http.request.method': method,
        'http.route': route,
        'http.response.status_code': statusCode,
    }
}

// Re-export the validated interval parser from the shared MJS utility so
// instrumentation.mjs and TypeScript code share the same implementation.
export { parseMetricExportInterval } from './otel-utils.mjs'

// ---------------------------------------------------------------------------
// Tracer / context helpers
// ---------------------------------------------------------------------------

export function getTracer(name: string = 'omni-web') {
    return trace.getTracer(name)
}

export function injectTraceContext(headers: Record<string, string>): Record<string, string> {
    const activeContext = context.active()
    const carrier: Record<string, string> = { ...headers }

    propagation.inject(activeContext, carrier)

    return carrier
}

export function getRequestId(): string | undefined {
    const span = trace.getActiveSpan()
    if (span) {
        return span.spanContext().traceId
    }
    return undefined
}

export function startSpan(name: string, fn: (span: Span) => Promise<any>) {
    const tracer = getTracer()
    return tracer.startActiveSpan(name, async (span) => {
        try {
            const result = await fn(span)
            span.end()
            return result
        } catch (error) {
            span.recordException(error as Error)
            span.end()
            throw error
        }
    })
}

// ---------------------------------------------------------------------------
// HTTP RED metrics instruments
// ---------------------------------------------------------------------------

const meter = metrics.getMeter('omni-web-http')

const httpRequestCounter = meter.createCounter('omni.http.server.request_count', {
    description: 'Total number of HTTP server requests by method, route, status',
})

const httpRequestDuration = meter.createHistogram('omni.http.server.request_duration_seconds', {
    description: 'HTTP server request duration in seconds',
    unit: 's',
})

export function recordHttpRequest(
    method: string,
    route: string,
    statusCode: number,
    durationSeconds: number,
) {
    const attributes = buildRedAttributes(method, route, statusCode)

    httpRequestCounter.add(1, attributes)
    httpRequestDuration.record(durationSeconds, attributes)
}
