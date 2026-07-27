/**
 * OpenTelemetry instrumentation bootstrap.
 *
 * Preloaded via Node.js `--import` flag so OTel SDK initialises before
 * any instrumented module (adapter-node, Vite, SvelteKit, Pino, etc.).
 *
 * This file is kept as plain .mjs so it can be loaded by Node directly
 * without a bundler or TypeScript transform.
 *
 * Shutdown sequencing
 * -------------------
 * In production (adapter-node), SIGTERM/SIGINT are handled by adapter-node
 * itself: it drains HTTP connections first, then emits `sveltekit:shutdown`.
 * Telemetry shutdown runs on that event so spans from the drain phase are
 * flushed.
 *
 * In dev/preview (Vite), there is no adapter-node, so SIGTERM/SIGINT are
 * handled directly here as a fallback.
 */

import { createSdk } from './otel-factory.mjs'

const otlpEndpointRaw = process.env.OTEL_EXPORTER_OTLP_ENDPOINT
const otlpEndpoint = otlpEndpointRaw ? otlpEndpointRaw.replace(/\/+$/, '') : undefined
const deploymentId = process.env.OTEL_DEPLOYMENT_ID || 'unknown'
const environment = process.env.OTEL_DEPLOYMENT_ENVIRONMENT || 'development'

const { sdk } = createSdk()

sdk.start()

if (otlpEndpoint) {
    console.log('OpenTelemetry initialized with OTLP endpoint configured')
} else {
    console.log('No OTLP endpoint configured, telemetry will be collected locally only')
}

console.log(
    `Telemetry initialized for omni-web (deployment_id=${deploymentId}, environment=${environment})`,
)

let didShutdown = false

const shutdown = async () => {
    if (didShutdown) return
    didShutdown = true
    try {
        await sdk.shutdown()
        console.log('Telemetry shut down successfully')
    } catch {
        console.error('Error shutting down telemetry')
    }
}

const isProduction = process.env.NODE_ENV === 'production'

// In production, adapter-node drains connections then emits sveltekit:shutdown.
// Listen on that event so spans from in-flight drain requests are flushed.
if (isProduction) {
    process.on('sveltekit:shutdown', shutdown)
} else {
    // Dev/preview fallback — no adapter-node, so handle signals directly.
    process.on('SIGTERM', shutdown)
    process.on('SIGINT', shutdown)
}
