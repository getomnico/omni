import { getGlobal, setGlobal } from './db/configuration'
import { getRedisClient } from './redis'

const SYSTEM_FLAGS_KEY = 'system:flags'
const SYSTEM_SETTINGS_KEY = 'system:settings'
const DOCLING_ENABLED_KEY = 'docling_enabled'
const DOCLING_QUALITY_PRESET_KEY = 'docling_quality_preset'
const DEFAULT_DOCLING_QUALITY_PRESET = 'balanced'

function booleanFromConfig(value: Record<string, unknown> | null, field: string): boolean | null {
    if (!value) return null
    const raw = value[field]
    if (typeof raw === 'boolean') return raw
    if (typeof raw === 'string') return raw === 'true'
    return null
}

function stringFromConfig(value: Record<string, unknown> | null, field: string): string | null {
    if (!value) return null
    const raw = value[field]
    return typeof raw === 'string' && raw.length > 0 ? raw : null
}

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
 * The source of truth is the global-scope `configuration` table. Redis is
 * retained only as a temporary compatibility/backfill path for older installs.
 */
export class SystemSettings {
    private static memoryCache: Map<string, string> = new Map()

    /**
     * Check if Docling-based document conversion is enabled.
     */
    static async isDoclingEnabled(): Promise<boolean> {
        if (this.memoryCache.has(DOCLING_ENABLED_KEY)) {
            return this.memoryCache.get(DOCLING_ENABLED_KEY) === 'true'
        }

        const config = await getGlobal(DOCLING_ENABLED_KEY)
        const configured = booleanFromConfig(config, 'enabled')
        if (configured !== null) {
            this.memoryCache.set(DOCLING_ENABLED_KEY, configured ? 'true' : 'false')
            return configured
        }

        // Compatibility/backfill from the old Redis hash.
        const redis = await getRedisClient()
        const value = await redis.hGet(SYSTEM_SETTINGS_KEY, DOCLING_ENABLED_KEY)
        const enabled = value === 'true'
        await setGlobal(DOCLING_ENABLED_KEY, { enabled })

        this.memoryCache.set(DOCLING_ENABLED_KEY, enabled ? 'true' : 'false')

        return enabled
    }

    /**
     * Set whether Docling-based document conversion is enabled.
     */
    static async setDoclingEnabled(enabled: boolean): Promise<void> {
        await setGlobal(DOCLING_ENABLED_KEY, { enabled })

        // Best-effort compatibility cache for older services during rollout.
        const redis = await getRedisClient()
        await redis.hSet(SYSTEM_SETTINGS_KEY, DOCLING_ENABLED_KEY, enabled ? 'true' : 'false')

        this.memoryCache.set(DOCLING_ENABLED_KEY, enabled ? 'true' : 'false')
    }

    /**
     * Get the Docling quality preset. Defaults to "balanced".
     */
    static async getDoclingQualityPreset(): Promise<string> {
        if (this.memoryCache.has(DOCLING_QUALITY_PRESET_KEY)) {
            return this.memoryCache.get(DOCLING_QUALITY_PRESET_KEY)!
        }

        const config = await getGlobal(DOCLING_QUALITY_PRESET_KEY)
        const configured = stringFromConfig(config, 'preset')
        if (configured) {
            this.memoryCache.set(DOCLING_QUALITY_PRESET_KEY, configured)
            return configured
        }

        // Compatibility/backfill from the old Redis hash.
        const redis = await getRedisClient()
        const value = await redis.hGet(SYSTEM_SETTINGS_KEY, DOCLING_QUALITY_PRESET_KEY)
        const preset = value ?? DEFAULT_DOCLING_QUALITY_PRESET
        await setGlobal(DOCLING_QUALITY_PRESET_KEY, { preset })

        this.memoryCache.set(DOCLING_QUALITY_PRESET_KEY, preset)

        return preset
    }

    /**
     * Set the Docling quality preset.
     */
    static async setDoclingQualityPreset(preset: string): Promise<void> {
        await setGlobal(DOCLING_QUALITY_PRESET_KEY, { preset })

        // Best-effort compatibility cache for older services during rollout.
        const redis = await getRedisClient()
        await redis.hSet(SYSTEM_SETTINGS_KEY, DOCLING_QUALITY_PRESET_KEY, preset)

        this.memoryCache.set(DOCLING_QUALITY_PRESET_KEY, preset)
    }

    /**
     * Clear memory cache
     */
    static clearCache(): void {
        this.memoryCache.clear()
    }
}
