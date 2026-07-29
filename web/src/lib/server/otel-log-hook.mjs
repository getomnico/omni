/**
 * Pino `hooks.logMethod` hook that exports log records to the OpenTelemetry
 * Logs API exactly once per Pino call.
 *
 * This replaces the auto-instrumentation's log-sending stream, which is
 * disabled in otel-factory.mjs via `disableLogSending: true`.  The hook
 * approach avoids module-load ordering issues because it is wired directly
 * into the Pino config in logger.ts rather than depending on patching.
 *
 * Native trace/span context is carried by the active OTel Context — the hook
 * does NOT inject correlation fields; that is handled separately by the
 * Pino `mixin` in logger.ts.
 *
 * Attribute filtering:
 * 1. Only string / number / boolean values are forwarded.
 *    Nested objects, Error instances, URL objects, arrays, and null are
 *    silently skipped.
 * 2. A strict explicit allowlist controls which attribute keys are
 *    permitted.  Any key not on the allowlist is rejected.
 *    Matching is case-insensitive.
 *
 * @module otel-log-hook
 */

import { logs, SeverityNumber } from '@opentelemetry/api-logs'

// ---------------------------------------------------------------------------
// Severity mapping
// ---------------------------------------------------------------------------

/** @type {Record<number, import('@opentelemetry/api-logs').SeverityNumber>} */
const PINO_LEVEL_TO_SEVERITY = {
    10: SeverityNumber.TRACE,
    20: SeverityNumber.DEBUG,
    30: SeverityNumber.INFO,
    40: SeverityNumber.WARN,
    50: SeverityNumber.ERROR,
    60: SeverityNumber.FATAL,
}

/** @type {Record<number, string>} */
const PINO_LEVEL_TO_TEXT = {
    10: 'TRACE',
    20: 'DEBUG',
    30: 'INFO',
    40: 'WARN',
    50: 'ERROR',
    60: 'FATAL',
}

// ---------------------------------------------------------------------------
// Attribute allowlist
// ---------------------------------------------------------------------------

/**
 * Strict explicit allowlist of permitted attribute keys (lowercase).
 * Any key not in this set is rejected.  Matching is case-insensitive.
 */
const ALLOWLIST = new Set([
    'method',
    'route',
    'status',
    'statuscode',
    'duration',
    'durationms',
    'count',
    'resultscount',
    'querylength',
    'contentlength',
    'mode',
    'provider',
    'operation',
    'outcome',
    'level',
    'module',
    'synctype',
    'mimetype',
    'size',
    'bytes',
])

/**
 * Check whether a key is permitted by the explicit allowlist.
 * Unknown keys are always rejected.
 * @param {string} key
 * @returns {boolean}
 */
function isKeyPermitted(key) {
    const lower = key.toLowerCase()

    // Only allowlisted keys are permitted
    return ALLOWLIST.has(lower)
}

// ---------------------------------------------------------------------------
// Safe attribute extraction
// ---------------------------------------------------------------------------

/**
 * Extract only safe primitive attributes from a log object.
 * Returns a new object containing only string / number / boolean values.
 * Nested objects, arrays, Error instances, null, and undefined are omitted.
 *
 * Attribute names are further filtered through a strict explicit allowlist
 * to prevent sensitive or high-cardinality data from reaching the OTel log
 * exporter.
 *
 * @param {Record<string, unknown> | undefined | null} obj
 * @returns {Record<string, string | number | boolean>}
 */
/** @param {Record<string, unknown> | undefined | null} obj */
export function extractSafeAttributes(obj) {
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) {
        return {}
    }

    /** @type {Record<string, string | number | boolean>} */
    const attrs = {}

    for (const key of Object.keys(obj)) {
        const val = obj[key]
        if (typeof val === 'string' || typeof val === 'number' || typeof val === 'boolean') {
            if (isKeyPermitted(key)) {
                attrs[key] = val
            }
        }
        // Skip: objects, arrays, Error, URL, null, undefined, bigint, symbol, function
    }

    return attrs
}

// ---------------------------------------------------------------------------
// Hook factory
// ---------------------------------------------------------------------------

/**
 * Create a Pino `hooks.logMethod` function.
 *
 * The returned function conforms to:
 *   (this: Logger, args: Parameters<LogFn>, method: LogFn, level: number) => void
 *
 * It emits one OTel LogRecord per invocation, then calls the original method
 * so the log line also reaches stdout.
 *
 * @returns {(args: unknown[], method: Function, level: number) => void}
 */
export function createPinoOtelHook() {
    const otelLogger = logs.getLogger('omni-web')

    return /** @this {any} @param {unknown[]} args @param {Function} method @param {number} level */ function pinoOtelLogMethodHook(args, method, level) {
        // Parse arguments — Pino calling conventions:
        //   logger.info('message')
        //   logger.info({ obj }, 'message')
        //   logger.info({ obj })
        let body = ''
        /** @type {Record<string, unknown>} */
        let bindings = {}

        if (args.length > 0) {
            const first = args[0]
            const second = args[1]

            if (typeof first === 'string') {
                // logger.info('message') or logger.info('message %s', ...)
                body = first
            } else if (first !== null && typeof first === 'object' && !Array.isArray(first)) {
                // logger.info({ obj }, 'message') or logger.info({ obj })
                bindings = /** @type {Record<string, unknown>} */ (first)
                if (typeof second === 'string') {
                    body = second
                }
            }
        }

        // Skip OTel export when otel_skip is set on the bindings object.
        // The log line still reaches stdout via method.apply below.
        if (bindings.otel_skip === true) {
            return method.apply(this, args)
        }

        // Map severity
        const severityNumber = PINO_LEVEL_TO_SEVERITY[level] ?? SeverityNumber.INFO
        const severityText = PINO_LEVEL_TO_TEXT[level] ?? 'INFO'

        // Extract only safe primitive attributes
        const attributes = extractSafeAttributes(bindings)

        // Emit one OTel log record
        otelLogger.emit({
            body,
            severityNumber,
            severityText,
            attributes,
        })

        // Call original Pino method so the log line also reaches stdout
        return method.apply(this, args)
    }
}
