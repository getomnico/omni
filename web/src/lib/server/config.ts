import { env } from '$env/dynamic/private'
import { createLogger } from './logger.js'

const logger = createLogger('config')

export interface AppConfig {
    database: {
        url: string
    }
    redis: {
        url: string
    }
    services: {
        searcherUrl: string
        indexerUrl: string
        aiServiceUrl: string
        connectorManagerUrl: string
        googleConnectorUrl: string | undefined
        slackConnectorUrl: string | undefined
        atlassianConnectorUrl: string | undefined
        webConnectorUrl: string | undefined
    }
    session: {
        secret: string
        cookieName: string
        durationDays: number
    }
    app: {
        publicUrl: string
    }
}

function getRequiredEnv(key: string): string {
    const value = env[key]
    if (!value) {
        logger.fatal(`Required environment variable '${key}' is not set`, undefined, {
            variable: key,
            message: 'Please set this variable in your .env file or environment',
        })
        process.exit(1)
    }
    return value
}

function getOptionalEnv(key: string, defaultValue: string): string {
    return env[key] || defaultValue
}

function validateUrl(url: string, name: string): string {
    try {
        new URL(url)
        return url
    } catch {
        logger.fatal(`Invalid URL for ${name}`, undefined, { name, url })
        process.exit(1)
    }
}

function validatePositiveNumber(value: string, name: string): number {
    const num = parseInt(value, 10)
    if (isNaN(num) || num <= 0) {
        logger.fatal(`${name} must be a positive number`, undefined, { name, value })
        process.exit(1)
    }
    return num
}

export function constructDatabaseUrl(): string {
    const databaseHost = getRequiredEnv('DATABASE_HOST')
    const databaseUsername = getRequiredEnv('DATABASE_USERNAME')
    const databaseName = getRequiredEnv('DATABASE_NAME')
    const databasePassword = getRequiredEnv('DATABASE_PASSWORD')
    const databasePort = getOptionalEnv('DATABASE_PORT', '5432')
    const requireSsl = getOptionalEnv('DATABASE_SSL', 'false') === 'true'

    const port = validatePositiveNumber(databasePort, 'DATABASE_PORT')

    const url = new URL(`postgresql://${databaseHost}:${port}/${databaseName}`)
    url.username = databaseUsername
    url.password = databasePassword

    if (requireSsl) {
        url.searchParams.set('sslmode', 'require')
    }

    return url.toString()
}

// Load and validate configuration
function loadConfig(): AppConfig {
    // Skip config validation during build time
    if (process.env.NODE_ENV === 'production' && !process.env.REDIS_URL) {
        logger.info('Skipping configuration validation during build')
        return {
            database: { url: 'postgresql://placeholder' },
            redis: { url: 'redis://placeholder' },
            services: {
                searcherUrl: 'http://placeholder',
                indexerUrl: 'http://placeholder',
                aiServiceUrl: 'http://placeholder',
                connectorManagerUrl: 'http://placeholder',
                googleConnectorUrl: undefined,
                slackConnectorUrl: undefined,
                atlassianConnectorUrl: undefined,
                webConnectorUrl: undefined,
            },
            session: {
                secret: 'placeholder',
                cookieName: 'auth-session',
                durationDays: 7,
            },
            app: { publicUrl: 'http://placeholder' },
        }
    }

    logger.info('Loading and validating application configuration')

    // Database configuration
    const databaseUrl = constructDatabaseUrl()
    validateUrl(databaseUrl, 'DATABASE_URL')

    // Redis configuration
    const redisUrl = getRequiredEnv('REDIS_URL')
    validateUrl(redisUrl, 'REDIS_URL')

    // Service URLs
    const searcherUrl = getRequiredEnv('SEARCHER_URL')
    const indexerUrl = getRequiredEnv('INDEXER_URL')
    const aiServiceUrl = getRequiredEnv('AI_SERVICE_URL')
    const connectorManagerUrl = getRequiredEnv('CONNECTOR_MANAGER_URL')
    const googleConnectorUrl = env.GOOGLE_CONNECTOR_URL || undefined
    const slackConnectorUrl = env.SLACK_CONNECTOR_URL || undefined
    const atlassianConnectorUrl = env.ATLASSIAN_CONNECTOR_URL || undefined
    const webConnectorUrl = env.WEB_CONNECTOR_URL || undefined
    validateUrl(searcherUrl, 'SEARCHER_URL')
    validateUrl(indexerUrl, 'INDEXER_URL')
    validateUrl(aiServiceUrl, 'AI_SERVICE_URL')
    validateUrl(connectorManagerUrl, 'CONNECTOR_MANAGER_URL')

    // Session configuration
    const sessionCookieName = getOptionalEnv('SESSION_COOKIE_NAME', 'auth-session')
    const sessionDurationDays = validatePositiveNumber(
        getOptionalEnv('SESSION_DURATION_DAYS', '7'),
        'SESSION_DURATION_DAYS',
    )

    // App configuration
    const publicAppUrl = getRequiredEnv('APP_URL')
    validateUrl(publicAppUrl, 'APP_URL')

    logger.info('Configuration validation completed successfully')

    return {
        database: {
            url: databaseUrl,
        },
        redis: {
            url: redisUrl,
        },
        services: {
            searcherUrl,
            indexerUrl,
            aiServiceUrl,
            connectorManagerUrl,
            googleConnectorUrl,
            slackConnectorUrl,
            atlassianConnectorUrl,
            webConnectorUrl,
        },
        session: {
            cookieName: sessionCookieName,
            durationDays: sessionDurationDays,
        },
        app: {
            publicUrl: publicAppUrl,
        },
    }
}

// Export configuration loading function and lazy-loaded config
let _config: AppConfig | null = null

export function getConfig(): AppConfig {
    if (!_config) {
        _config = loadConfig()
    }
    return _config
}

// For backward compatibility, export config as a getter
export const config = getConfig()

// Also export individual sections for convenience
export const { database, redis, services, session, app } = config
