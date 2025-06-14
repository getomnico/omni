import { env } from '$env/dynamic/private';

export interface AppConfig {
	database: {
		url: string;
	};
	redis: {
		url: string;
	};
	services: {
		searcherUrl: string;
		indexerUrl: string;
		aiServiceUrl: string;
	};
	session: {
		secret: string;
		cookieName: string;
		durationDays: number;
	};
	app: {
		publicUrl: string;
	};
	oauth: {
		google: {
			clientId: string;
			clientSecret: string;
			redirectUri: string;
		};
	};
}

function getRequiredEnv(key: string): string {
	const value = env[key];
	if (!value) {
		console.error(`ERROR: Required environment variable '${key}' is not set`);
		console.error('Please set this variable in your .env file or environment');
		process.exit(1);
	}
	return value;
}

function getOptionalEnv(key: string, defaultValue: string): string {
	return env[key] || defaultValue;
}

function validateUrl(url: string, name: string): string {
	try {
		new URL(url);
		return url;
	} catch {
		console.error(`ERROR: Invalid URL for ${name}: ${url}`);
		process.exit(1);
	}
}

function validatePositiveNumber(value: string, name: string): number {
	const num = parseInt(value, 10);
	if (isNaN(num) || num <= 0) {
		console.error(`ERROR: ${name} must be a positive number, got: ${value}`);
		process.exit(1);
	}
	return num;
}

function validateOAuthCredentials(clientId: string, clientSecret: string, provider: string): void {
	const defaultIds = [`your-${provider}-client-id`, `your-${provider}-client-secret`];
	
	if (!clientId || !clientSecret) {
		console.error(`ERROR: ${provider} OAuth credentials are not set`);
		console.error(`Please set ${provider.toUpperCase()}_CLIENT_ID and ${provider.toUpperCase()}_CLIENT_SECRET`);
		process.exit(1);
	}
	
	if (defaultIds.includes(clientId) || defaultIds.includes(clientSecret)) {
		console.error(`ERROR: ${provider} OAuth credentials are using default placeholder values`);
		console.error(`Please configure your ${provider} OAuth credentials`);
		process.exit(1);
	}
}

// Load and validate configuration
function loadConfig(): AppConfig {
	// Skip config validation during build time
	if (process.env.NODE_ENV === 'production' && !process.env.REDIS_URL) {
		console.log('Skipping configuration validation during build...');
		return {
			database: { url: 'postgresql://placeholder' },
			redis: { url: 'redis://placeholder' },
			services: {
				searcherUrl: 'http://placeholder',
				indexerUrl: 'http://placeholder',
				aiServiceUrl: 'http://placeholder'
			},
			session: {
				secret: 'placeholder',
				cookieName: 'auth-session',
				durationDays: 7
			},
			app: { publicUrl: 'http://placeholder' },
			oauth: {
				google: {
					clientId: 'placeholder',
					clientSecret: 'placeholder',
					redirectUri: 'http://placeholder'
				}
			}
		};
	}

	console.log('Loading and validating application configuration...');

	// Database configuration
	const databaseUrl = getRequiredEnv('DATABASE_URL');
	validateUrl(databaseUrl, 'DATABASE_URL');

	// Redis configuration
	const redisUrl = getRequiredEnv('REDIS_URL');
	validateUrl(redisUrl, 'REDIS_URL');

	// Service URLs
	const searcherUrl = getRequiredEnv('SEARCHER_URL');
	const indexerUrl = getRequiredEnv('INDEXER_URL');
	const aiServiceUrl = getRequiredEnv('AI_SERVICE_URL');
	validateUrl(searcherUrl, 'SEARCHER_URL');
	validateUrl(indexerUrl, 'INDEXER_URL');
	validateUrl(aiServiceUrl, 'AI_SERVICE_URL');

	// Session configuration
	const sessionSecret = getRequiredEnv('SESSION_SECRET');
	if (sessionSecret === 'your-session-secret-key-change-in-production') {
		console.error('ERROR: SESSION_SECRET is using the default value');
		console.error('Please set a secure session secret in production');
		process.exit(1);
	}
	
	const sessionCookieName = getOptionalEnv('SESSION_COOKIE_NAME', 'auth-session');
	const sessionDurationDays = validatePositiveNumber(
		getOptionalEnv('SESSION_DURATION_DAYS', '7'),
		'SESSION_DURATION_DAYS'
	);

	// App configuration
	const publicAppUrl = getRequiredEnv('PUBLIC_APP_URL');
	validateUrl(publicAppUrl, 'PUBLIC_APP_URL');

	// Google OAuth configuration
	const googleClientId = getRequiredEnv('GOOGLE_CLIENT_ID');
	const googleClientSecret = getRequiredEnv('GOOGLE_CLIENT_SECRET');
	const googleRedirectUri = getRequiredEnv('GOOGLE_REDIRECT_URI');
	
	validateOAuthCredentials(googleClientId, googleClientSecret, 'google');
	validateUrl(googleRedirectUri, 'GOOGLE_REDIRECT_URI');

	console.log('Configuration validation completed successfully');

	return {
		database: {
			url: databaseUrl
		},
		redis: {
			url: redisUrl
		},
		services: {
			searcherUrl,
			indexerUrl,
			aiServiceUrl
		},
		session: {
			secret: sessionSecret,
			cookieName: sessionCookieName,
			durationDays: sessionDurationDays
		},
		app: {
			publicUrl: publicAppUrl
		},
		oauth: {
			google: {
				clientId: googleClientId,
				clientSecret: googleClientSecret,
				redirectUri: googleRedirectUri
			}
		}
	};
}

// Export configuration loading function and lazy-loaded config
let _config: AppConfig | null = null;

export function getConfig(): AppConfig {
	if (!_config) {
		_config = loadConfig();
	}
	return _config;
}

// For backward compatibility, export config as a getter
export const config = getConfig();

// Also export individual sections for convenience
export const { database, redis, services, session, app, oauth } = config;