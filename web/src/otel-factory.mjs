/**
 * OpenTelemetry SDK factory — creates a NodeSDK instance.
 *
 * Separated from instrumentation.mjs so tests can create an SDK against
 * a local collector without bootstrapping the whole process.
 */

import { NodeSDK } from '@opentelemetry/sdk-node'
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http'
import { OTLPMetricExporter } from '@opentelemetry/exporter-metrics-otlp-http'
import { OTLPLogExporter } from '@opentelemetry/exporter-logs-otlp-http'
import { PeriodicExportingMetricReader } from '@opentelemetry/sdk-metrics'
import { BatchLogRecordProcessor } from '@opentelemetry/sdk-logs'
import { resourceFromAttributes } from '@opentelemetry/resources'
import { ATTR_SERVICE_NAME, ATTR_SERVICE_VERSION } from '@opentelemetry/semantic-conventions'
import { getNodeAutoInstrumentations } from '@opentelemetry/auto-instrumentations-node'
import { parseMetricExportInterval } from './lib/server/otel-utils.mjs'

/**
 * Build a NodeSDK instance from environment variables.
 *
 * @param {object} [overrides] - override environment-derived values for testing
 * @param {string} [overrides.otlpEndpoint] - OTLP HTTP endpoint (default: OTEL_EXPORTER_OTLP_ENDPOINT env)
 * @param {string} [overrides.serviceName] - service name (default: 'omni-web')
 * @returns {{ sdk: NodeSDK, logRecordProcessors: import('@opentelemetry/sdk-logs').BatchLogRecordProcessor[] | undefined }}
 */
export function createSdk(overrides = {}) {
    const otlpEndpointRaw = overrides.otlpEndpoint ?? process.env.OTEL_EXPORTER_OTLP_ENDPOINT
    const otlpEndpoint = otlpEndpointRaw ? otlpEndpointRaw.replace(/\/+$/, '') : undefined

    const deploymentId = process.env.OTEL_DEPLOYMENT_ID || 'unknown'
    const environment = process.env.OTEL_DEPLOYMENT_ENVIRONMENT || 'development'
    const serviceVersion = process.env.SERVICE_VERSION || '0.1.0'
    const serviceName = overrides.serviceName || 'omni-web'

    const resource = resourceFromAttributes({
        [ATTR_SERVICE_NAME]: serviceName,
        [ATTR_SERVICE_VERSION]: serviceVersion,
        'deployment.environment': environment,
        'deployment.id': deploymentId,
    })

    const traceExporter = otlpEndpoint
        ? new OTLPTraceExporter({
              url: `${otlpEndpoint}/v1/traces`,
          })
        : undefined

    const metricExportInterval = parseMetricExportInterval(process.env.OTEL_METRIC_EXPORT_INTERVAL)
    const metricReader = otlpEndpoint
        ? new PeriodicExportingMetricReader({
              exporter: new OTLPMetricExporter({
                  url: `${otlpEndpoint}/v1/metrics`,
              }),
              exportIntervalMillis: metricExportInterval,
              exportTimeoutMillis: Math.min(metricExportInterval / 2, 30000),
          })
        : undefined

    const logRecordProcessors = otlpEndpoint
        ? [
              new BatchLogRecordProcessor({
                  exporter: new OTLPLogExporter({
                      url: `${otlpEndpoint}/v1/logs`,
                  }),
              }),
          ]
        : undefined

    const sdk = new NodeSDK({
        resource,
        traceExporter,
        metricReader,
        logRecordProcessors,
        instrumentations: [
            getNodeAutoInstrumentations({
                '@opentelemetry/instrumentation-fs': {
                    enabled: false,
                },
                '@opentelemetry/instrumentation-pino': {
                    disableLogSending: true,
                },
            }),
        ],
    })

    return { sdk, logRecordProcessors }
}
