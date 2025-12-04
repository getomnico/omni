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
        googleConnectorUrl: string
        slackConnectorUrl: string
        atlassianConnectorUrl: string
        webConnectorUrl: string
    }
    session: {
        secret: string
        cookieName: string
        durationDays: number
    }
    app: {
        publicUrl: string
    }
    oauth: {
        google: {
            clientId: string
            clientSecret: string
            redirectUri: string
            scopes: string[]
        }
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
                googleConnectorUrl: 'http://placeholder',
                slackConnectorUrl: 'http://placeholder',
                atlassianConnectorUrl: 'http://placeholder',
                webConnectorUrl: 'http://placeholder',
            },
            session: {
                secret: 'placeholder',
                cookieName: 'auth-session',
                durationDays: 7,
            },
            app: { publicUrl: 'http://placeholder' },
            oauth: {
                google: {
                    clientId: 'placeholder',
                    clientSecret: 'placeholder',
                    redirectUri: 'http://placeholder',
                    scopes: ['openid', 'profile', 'email'],
                },
            },
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
    const googleConnectorUrl = getRequiredEnv('GOOGLE_CONNECTOR_URL')
    const slackConnectorUrl = getRequiredEnv('SLACK_CONNECTOR_URL')
    const atlassianConnectorUrl = getRequiredEnv('ATLASSIAN_CONNECTOR_URL')
    const webConnectorUrl = getRequiredEnv('WEB_CONNECTOR_URL')
    validateUrl(searcherUrl, 'SEARCHER_URL')
    validateUrl(indexerUrl, 'INDEXER_URL')
    validateUrl(aiServiceUrl, 'AI_SERVICE_URL')
    validateUrl(googleConnectorUrl, 'GOOGLE_CONNECTOR_URL')
    validateUrl(slackConnectorUrl, 'SLACK_CONNECTOR_URL')
    validateUrl(atlassianConnectorUrl, 'ATLASSIAN_CONNECTOR_URL')
    validateUrl(webConnectorUrl, 'WEB_CONNECTOR_URL')

    // Session configuration
    const sessionSecret = getRequiredEnv('SESSION_SECRET')
    const sessionCookieName = getOptionalEnv('SESSION_COOKIE_NAME', 'auth-session')
    const sessionDurationDays = validatePositiveNumber(
        getOptionalEnv('SESSION_DURATION_DAYS', '7'),
        'SESSION_DURATION_DAYS',
    )

    // App configuration
    const publicAppUrl = getRequiredEnv('APP_URL')
    validateUrl(publicAppUrl, 'APP_URL')

    // OAuth configuration
    const googleOAuthClientId = getOptionalEnv('GOOGLE_OAUTH_CLIENT_ID', '')
    const googleOAuthClientSecret = getOptionalEnv('GOOGLE_OAUTH_CLIENT_SECRET', '')
    const googleOAuthRedirectUri = getOptionalEnv(
        'GOOGLE_OAUTH_REDIRECT_URI',
        `${publicAppUrl}/auth/google/callback`,
    )
    const googleOAuthScopes = getOptionalEnv('GOOGLE_OAUTH_SCOPES', 'openid,profile,email').split(
        ',',
    )

    // Validate OAuth redirect URI
    if (googleOAuthClientId && googleOAuthRedirectUri) {
        validateUrl(googleOAuthRedirectUri, 'GOOGLE_OAUTH_REDIRECT_URI')
    }

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
            googleConnectorUrl,
            slackConnectorUrl,
            atlassianConnectorUrl,
            webConnectorUrl,
        },
        session: {
            secret: sessionSecret,
            cookieName: sessionCookieName,
            durationDays: sessionDurationDays,
        },
        app: {
            publicUrl: publicAppUrl,
        },
        oauth: {
            google: {
                clientId: googleOAuthClientId,
                clientSecret: googleOAuthClientSecret,
                redirectUri: googleOAuthRedirectUri,
                scopes: googleOAuthScopes,
            },
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
export const { database, redis, services, session, app, oauth } = config
