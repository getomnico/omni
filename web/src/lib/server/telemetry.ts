import { NodeSDK } from '@opentelemetry/sdk-node'
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http'
import { Resource } from '@opentelemetry/resources'
import { ATTR_SERVICE_NAME, ATTR_SERVICE_VERSION } from '@opentelemetry/semantic-conventions'
import { getNodeAutoInstrumentations } from '@opentelemetry/auto-instrumentations-node'
import { ulid } from 'ulid'
import { propagation, trace, context, type Span } from '@opentelemetry/api'

let sdk: NodeSDK | null = null

export function initTelemetry() {
    const otlpEndpoint = process.env.OTEL_EXPORTER_OTLP_ENDPOINT
    const deploymentId = process.env.OTEL_DEPLOYMENT_ID || ulid()
    const environment = process.env.OTEL_DEPLOYMENT_ENVIRONMENT || 'development'
    const serviceVersion = process.env.SERVICE_VERSION || '0.1.0'

    const resource = new Resource({
        [ATTR_SERVICE_NAME]: 'omni-web',
        [ATTR_SERVICE_VERSION]: serviceVersion,
        'deployment.environment': environment,
        'deployment.id': deploymentId,
    })

    const traceExporter = otlpEndpoint
        ? new OTLPTraceExporter({
              url: `${otlpEndpoint}/v1/traces`,
          })
        : undefined

    sdk = new NodeSDK({
        resource,
        traceExporter,
        instrumentations: [
            getNodeAutoInstrumentations({
                '@opentelemetry/instrumentation-fs': {
                    enabled: false,
                },
            }),
        ],
    })

    sdk.start()

    if (otlpEndpoint) {
        console.log(`OpenTelemetry initialized with OTLP endpoint: ${otlpEndpoint}`)
    } else {
        console.log('No OTLP endpoint configured, telemetry will be collected locally only')
    }

    console.log(
        `Telemetry initialized for omni-web (deployment_id=${deploymentId}, environment=${environment})`,
    )

    // Graceful shutdown
    process.on('SIGTERM', async () => {
        try {
            await sdk?.shutdown()
            console.log('Telemetry shut down successfully')
        } catch (error) {
            console.error('Error shutting down telemetry', error)
        } finally {
            process.exit(0)
        }
    })
}

export function getTracer(name: string = 'omni-web') {
    return trace.getTracer(name)
}

export function injectTraceContext(headers: Record<string, string>): Record<string, string> {
    const activeContext = context.active()
    const carrier: Record<string, string> = { ...headers }

    propagation.inject(activeContext, carrier)

    return carrier
}

export function extractTraceContext(headers: Record<string, string | undefined>) {
    const carrier: Record<string, string> = {}

    for (const [key, value] of Object.entries(headers)) {
        if (value !== undefined) {
            carrier[key] = value
        }
    }

    return propagation.extract(context.active(), carrier)
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
