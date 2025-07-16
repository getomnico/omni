import { env } from '$env/dynamic/private'

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
        console.error(`ERROR: Required environment variable '${key}' is not set`)
        console.error('Please set this variable in your .env file or environment')
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
        console.error(`ERROR: Invalid URL for ${name}: ${url}`)
        process.exit(1)
    }
}

function validatePositiveNumber(value: string, name: string): number {
    const num = parseInt(value, 10)
    if (isNaN(num) || num <= 0) {
        console.error(`ERROR: ${name} must be a positive number, got: ${value}`)
        process.exit(1)
    }
    return num
}

// Load and validate configuration
function loadConfig(): AppConfig {
    // Skip config validation during build time
    if (process.env.NODE_ENV === 'production' && !process.env.REDIS_URL) {
        console.log('Skipping configuration validation during build...')
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

    console.log('Loading and validating application configuration...')

    // Database configuration
    const databaseUrl = getRequiredEnv('DATABASE_URL')
    validateUrl(databaseUrl, 'DATABASE_URL')

    // Redis configuration
    const redisUrl = getRequiredEnv('REDIS_URL')
    validateUrl(redisUrl, 'REDIS_URL')

    // Service URLs
    const searcherUrl = getRequiredEnv('SEARCHER_URL')
    const indexerUrl = getRequiredEnv('INDEXER_URL')
    const aiServiceUrl = getRequiredEnv('AI_SERVICE_URL')
    const googleConnectorUrl = getRequiredEnv('GOOGLE_CONNECTOR_URL')
    const slackConnectorUrl = getOptionalEnv('SLACK_CONNECTOR_URL', 'http://slack-connector:4002')
    const atlassianConnectorUrl = getOptionalEnv(
        'ATLASSIAN_CONNECTOR_URL',
        'http://atlassian-connector:4003',
    )
    validateUrl(searcherUrl, 'SEARCHER_URL')
    validateUrl(indexerUrl, 'INDEXER_URL')
    validateUrl(aiServiceUrl, 'AI_SERVICE_URL')
    validateUrl(googleConnectorUrl, 'GOOGLE_CONNECTOR_URL')
    validateUrl(slackConnectorUrl, 'SLACK_CONNECTOR_URL')
    validateUrl(atlassianConnectorUrl, 'ATLASSIAN_CONNECTOR_URL')

    // Session configuration
    const sessionSecret = getRequiredEnv('SESSION_SECRET')
    if (sessionSecret === 'your-session-secret-key-change-in-production') {
        console.error('ERROR: SESSION_SECRET is using the default value')
        console.error('Please set a secure session secret in production')
        process.exit(1)
    }

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

    console.log('Configuration validation completed successfully')

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
