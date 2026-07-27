/**
 * Shared OpenTelemetry utilities for plain-JS bootstrap (instrumentation.mjs).
 *
 * Exports pure functions used by both the SDK bootstrap and TypeScript
 * production code.
 */

/**
 * Parse OTEL_METRIC_EXPORT_INTERVAL as a finite positive integer (milliseconds).
 * Defaults to 60000 when unset or invalid.
 * @param {string | undefined} intervalRaw
 * @returns {number}
 */
export function parseMetricExportInterval(intervalRaw) {
    if (intervalRaw === undefined) return 60_000;
    const parsed = parseInt(intervalRaw, 10);
    if (Number.isFinite(parsed) && parsed > 0) return parsed;
    return 60_000;
}

/**
 * Convert a duration from milliseconds to seconds.
 * @param {number} ms
 * @returns {number}
 */
export function millisecondsToSeconds(ms) {
    return ms / 1000;
}
