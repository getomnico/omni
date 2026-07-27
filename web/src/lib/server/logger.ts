import pino, { type Logger as PinoLogger } from 'pino'
import { trace } from '@opentelemetry/api'
import { env } from '$env/dynamic/private'
import { dev } from '$app/environment'
import { ulid } from 'ulid'
import { createPinoOtelHook } from './otel-log-hook.mjs'

const logLevel = env.LOG_LEVEL || (dev ? 'debug' : 'info')
const logPretty = env.LOG_PRETTY === 'true' || dev

const transport = logPretty
    ? {
          target: 'pino-pretty',
          options: {
              colorize: true,
              levelFirst: true,
              translateTime: 'yyyy-mm-dd HH:MM:ss.l',
              ignore: 'pid,hostname',
              messageFormat: '{msg}',
              errorLikeObjectKeys: ['err', 'error'],
              singleLine: true,
          },
      }
    : undefined

const productionHook = createPinoOtelHook()

const pinoConfig: pino.LoggerOptions = {
    level: logLevel,
    timestamp: pino.stdTimeFunctions.isoTime,
    formatters: {
        level: (label) => {
            return { level: label.toUpperCase() }
        },
    },
    serializers: {
        // Error serializer: emit only bounded error type/name.
        // Dynamic message (err.message) and stack first line are NOT
        // emitted — they are user-controlled and may contain sensitive
        // data.
        error: (err: Error) => ({
            type: err.name,
        }),
        // No request/response serializers — these dump headers, query,
        // params, and response bodies. Individual log calls pass explicit
        // bounded fields (method, route, status, duration).
    },
    /**
     * Inject real trace_id / span_id into every stdout Pino JSON line
     * when inside a valid OTel span.  This ensures stdout log correlation
     * works even if the auto-instrumentation mixin patch hasn't fired yet.
     */
    mixin(_context: object, _level: number): Record<string, string> {
        const span = trace.getActiveSpan()
        if (!span) return {}
        const spanContext = span.spanContext()
        return {
            trace_id: spanContext.traceId,
            span_id: spanContext.spanId,
        }
    },
    /**
     * Export each log record to OTel via the production hook exactly once,
     * independent of auto-instrumentation module-load quirks.
     */
    hooks: {
        logMethod: productionHook,
    },
    ...(transport && { transport }),
}

const baseLogger = pino(pinoConfig)

export class Logger {
    private logger: PinoLogger

    constructor(name?: string, metadata?: Record<string, unknown>) {
        this.logger = name
            ? baseLogger.child({ module: name, ...metadata })
            : baseLogger.child(metadata || {})
    }

    static generateRequestId(): string {
        return ulid()
    }

    child(name: string, metadata?: Record<string, unknown>): Logger {
        const childLogger = new Logger()
        childLogger.logger = this.logger.child({ module: name, ...metadata })
        return childLogger
    }

    withRequest(requestId: string, userId?: string): Logger {
        const childLogger = new Logger()
        childLogger.logger = this.logger.child({ requestId, userId })
        return childLogger
    }

    debug(message: string, data?: unknown): void {
        if (data) {
            // Safe normalization: Pino expects object | string as first arg
            this.logger.debug(data as object, message)
        } else {
            this.logger.debug(message)
        }
    }

    info(message: string, data?: unknown): void {
        if (data) {
            this.logger.info(data as object, message)
        } else {
            this.logger.info(message)
        }
    }

    warn(message: string, data?: unknown): void {
        if (data) {
            this.logger.warn(data as object, message)
        } else {
            this.logger.warn(message)
        }
    }

    error(message: string, error?: unknown, data?: Record<string, unknown>): void {
        if (error instanceof Error) {
            this.logger.error({ error, ...(data ?? {}) }, message)
        } else if (error) {
            // Non-Error second arg: existing behaviour passes it as data
            this.logger.error({ ...(data ?? {}) }, message)
        } else {
            this.logger.error(data ?? {}, message)
        }
    }

    fatal(message: string, error?: unknown, data?: Record<string, unknown>): void {
        if (error instanceof Error) {
            this.logger.fatal({ error, ...(data ?? {}) }, message)
        } else if (error) {
            this.logger.fatal({ ...(data ?? {}) }, message)
        } else {
            this.logger.fatal(data ?? {}, message)
        }
    }

    time(label: string): () => void {
        const start = Date.now()
        return () => {
            const duration = Date.now() - start
            this.logger.info({ duration }, `${label} completed`)
        }
    }
}

export const logger = new Logger('omni-web')

export function createLogger(name: string, metadata?: Record<string, unknown>): Logger {
    return new Logger(name, metadata)
}
