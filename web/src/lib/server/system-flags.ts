import {
    GLOBAL_CONFIGURATION_KEYS,
    getTypedGlobal,
    setTypedGlobal,
    type DoclingQualityPreset,
} from './db/configuration'
import { userRepository } from './db/users'
import { getRedisClient } from './redis'

const SYSTEM_FLAGS_KEY = 'system:flags'
const DOCLING_ENABLED_KEY = GLOBAL_CONFIGURATION_KEYS.DOCLING_ENABLED
const DOCLING_QUALITY_PRESET_KEY = GLOBAL_CONFIGURATION_KEYS.DOCLING_QUALITY_PRESET
const DEFAULT_DOCLING_QUALITY_PRESET: DoclingQualityPreset = 'balanced'

function isDoclingQualityPreset(preset: string): preset is DoclingQualityPreset {
    return preset === 'fast' || preset === 'balanced' || preset === 'quality'
}

export class SystemFlags {
    private static memoryCache: Map<string, boolean> = new Map()

    /**
     * Check if the system has been initialized (first admin created)
     */
    static async isInitialized(): Promise<boolean> {
        // A cached true is safe to trust. Do not trust cached false indefinitely: Redis may
        // have been cleared while Postgres still contains users (for example in dev), and
        // first-run setup should never be shown once accounts exist.
        if (this.memoryCache.get('initialized') === true) {
            return true
        }

        const redis = await getRedisClient()
        const value = await redis.hGet(SYSTEM_FLAGS_KEY, 'initialized')
        if (value === 'true') {
            this.memoryCache.set('initialized', true)
            return true
        }

        // Backstop the Redis flag with the database source of truth. If an admin exists,
        // repair the Redis flag and consider the system initialized.
        const hasAdminUsers = await userRepository.hasAnyAdminUsers()

        if (hasAdminUsers) {
            await redis.hSet(SYSTEM_FLAGS_KEY, 'initialized', 'true')
            this.memoryCache.set('initialized', true)
            return true
        }

        this.memoryCache.set('initialized', false)
        return false
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
 * The source of truth is the global-scope `configuration` table.
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

        const config = await getTypedGlobal(DOCLING_ENABLED_KEY)
        const enabled = config?.enabled ?? false
        this.memoryCache.set(DOCLING_ENABLED_KEY, enabled ? 'true' : 'false')
        return enabled
    }

    /**
     * Set whether Docling-based document conversion is enabled.
     */
    static async setDoclingEnabled(enabled: boolean): Promise<void> {
        await setTypedGlobal(DOCLING_ENABLED_KEY, { enabled })
        this.memoryCache.set(DOCLING_ENABLED_KEY, enabled ? 'true' : 'false')
    }

    /**
     * Get the Docling quality preset. Defaults to "balanced".
     */
    static async getDoclingQualityPreset(): Promise<string> {
        if (this.memoryCache.has(DOCLING_QUALITY_PRESET_KEY)) {
            return this.memoryCache.get(DOCLING_QUALITY_PRESET_KEY)!
        }

        const config = await getTypedGlobal(DOCLING_QUALITY_PRESET_KEY)
        const preset = config?.preset ?? DEFAULT_DOCLING_QUALITY_PRESET
        this.memoryCache.set(DOCLING_QUALITY_PRESET_KEY, preset)
        return preset
    }

    /**
     * Set the Docling quality preset.
     */
    static async setDoclingQualityPreset(preset: string): Promise<void> {
        if (!isDoclingQualityPreset(preset)) {
            throw new Error(`Invalid Docling quality preset: ${preset}`)
        }

        await setTypedGlobal(DOCLING_QUALITY_PRESET_KEY, { preset })
        this.memoryCache.set(DOCLING_QUALITY_PRESET_KEY, preset)
    }

    /**
     * Clear memory cache
     */
    static clearCache(): void {
        this.memoryCache.clear()
    }
}
