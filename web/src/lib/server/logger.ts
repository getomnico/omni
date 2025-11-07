import pino, { type Logger as PinoLogger } from 'pino'
import { env } from '$env/dynamic/private'
import { dev } from '$app/environment'
import { ulid } from 'ulid'

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

const pinoConfig: pino.LoggerOptions = {
    level: logLevel,
    timestamp: pino.stdTimeFunctions.isoTime,
    formatters: {
        level: (label) => {
            return { level: label.toUpperCase() }
        },
    },
    serializers: {
        error: (err: Error) => ({
            type: err.name,
            message: err.message,
            stack: err.stack,
        }),
        request: (req: any) => ({
            method: req.method,
            url: req.url,
            headers: req.headers,
            query: req.query,
            params: req.params,
        }),
        response: (res: any) => ({
            statusCode: res.statusCode,
            headers: res.headers,
        }),
    },
    ...(transport && { transport }),
}

const baseLogger = pino(pinoConfig)

export class Logger {
    private logger: PinoLogger

    constructor(name?: string, metadata?: Record<string, any>) {
        this.logger = name
            ? baseLogger.child({ module: name, ...metadata })
            : baseLogger.child(metadata || {})
    }

    static generateRequestId(): string {
        return ulid()
    }

    child(name: string, metadata?: Record<string, any>): Logger {
        const childLogger = new Logger()
        childLogger.logger = this.logger.child({ module: name, ...metadata })
        return childLogger
    }

    withRequest(requestId: string, userId?: string): Logger {
        const childLogger = new Logger()
        childLogger.logger = this.logger.child({ requestId, userId })
        return childLogger
    }

    debug(message: string, data?: any): void {
        if (data) {
            this.logger.debug(data, message)
        } else {
            this.logger.debug(message)
        }
    }

    info(message: string, data?: any): void {
        if (data) {
            this.logger.info(data, message)
        } else {
            this.logger.info(message)
        }
    }

    warn(message: string, data?: any): void {
        if (data) {
            this.logger.warn(data, message)
        } else {
            this.logger.warn(message)
        }
    }

    error(message: string, error?: Error | any, data?: any): void {
        if (error instanceof Error) {
            this.logger.error({ error, ...data }, message)
        } else if (error) {
            this.logger.error({ ...error, ...data }, message)
        } else {
            this.logger.error(data || {}, message)
        }
    }

    fatal(message: string, error?: Error | any, data?: any): void {
        if (error instanceof Error) {
            this.logger.fatal({ error, ...data }, message)
        } else if (error) {
            this.logger.fatal({ ...error, ...data }, message)
        } else {
            this.logger.fatal(data || {}, message)
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

export function createLogger(name: string, metadata?: Record<string, any>): Logger {
    return new Logger(name, metadata)
}
