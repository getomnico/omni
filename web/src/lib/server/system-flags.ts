import { getRedisClient } from './redis'

const SYSTEM_FLAGS_KEY = 'system:flags'
const SYSTEM_SETTINGS_KEY = 'system:settings'

export class SystemFlags {
    private static memoryCache: Map<string, boolean> = new Map()

    /**
     * Check if the system has been initialized (first admin created)
     */
    static async isInitialized(): Promise<boolean> {
        // Check memory cache first
        if (this.memoryCache.has('initialized')) {
            return this.memoryCache.get('initialized')!
        }

        // Check Redis
        const redis = await getRedisClient()
        const value = await redis.hGet(SYSTEM_FLAGS_KEY, 'initialized')
        const initialized = value === 'true'

        // Cache in memory
        this.memoryCache.set('initialized', initialized)

        return initialized
    }

    /**
     * Mark system as initialized (called after first admin account creation)
     */
    static async markAsInitialized(): Promise<void> {
        const redis = await getRedisClient()
        await redis.hSet(SYSTEM_FLAGS_KEY, 'initialized', 'true')
        this.memoryCache.set('initialized', true)
    }

    /**
     * Reset initialization flag (useful for testing)
     */
    static async resetInitialization(): Promise<void> {
        const redis = await getRedisClient()
        await redis.hDel(SYSTEM_FLAGS_KEY, 'initialized')
        this.memoryCache.delete('initialized')
    }

    /**
     * Clear memory cache (useful if Redis was updated externally)
     */
    static clearCache(): void {
        this.memoryCache.clear()
    }
}

/**
 * System settings that can be configured via the admin UI.
 * These are stored in Redis and can be changed at runtime.
 */
export class SystemSettings {
    private static memoryCache: Map<string, string> = new Map()

    /**
     * Check if Docling-based document conversion is enabled.
     * Environment variable DOCLING_ENABLED takes precedence over Redis setting.
     * An empty value is treated as unset (defers to Redis / UI setting).
     */
    static async isDoclingEnabled(): Promise<boolean> {
        // Environment variable takes precedence (non-empty values only)
        const envValue = process.env.DOCLING_ENABLED
        if (envValue !== undefined && envValue !== '') {
            return envValue.toLowerCase() === 'true'
        }

        // Check memory cache
        if (this.memoryCache.has('docling_enabled')) {
            return this.memoryCache.get('docling_enabled') === 'true'
        }

        // Check Redis
        const redis = await getRedisClient()
        const value = await redis.hGet(SYSTEM_SETTINGS_KEY, 'docling_enabled')
        const enabled = value === 'true'

        // Cache in memory
        this.memoryCache.set('docling_enabled', enabled ? 'true' : 'false')

        return enabled
    }

    /**
     * Set whether Docling-based document conversion is enabled.
     * Note: If DOCLING_ENABLED env var is set, it will still take precedence.
     */
    static async setDoclingEnabled(enabled: boolean): Promise<void> {
        const redis = await getRedisClient()
        await redis.hSet(SYSTEM_SETTINGS_KEY, 'docling_enabled', enabled ? 'true' : 'false')
        this.memoryCache.set('docling_enabled', enabled ? 'true' : 'false')
    }

    /**
     * Check if Docling setting is overridden by environment variable.
     * An empty value is treated as unset.
     */
    static isDoclingOverriddenByEnv(): boolean {
        const val = process.env.DOCLING_ENABLED
        return val !== undefined && val !== ''
    }

    /**
     * Clear memory cache
     */
    static clearCache(): void {
        this.memoryCache.clear()
    }
}
