import { browser } from '$app/environment'
import { DEFAULT_PREFERENCES, STORAGE_KEY, type UserPreferences } from './user-preferences'

class PreferencesStorage {
    private preferences = $state<UserPreferences>(DEFAULT_PREFERENCES)

    constructor() {
        if (browser) {
            this.load()
        }
    }

    private load(): void {
        try {
            const stored = localStorage.getItem(STORAGE_KEY)
            if (stored) {
                const parsed = JSON.parse(stored) as Partial<UserPreferences>
                if (parsed.theme === 'bright') {
                    parsed.theme = 'light'
                    parsed.themeColorScheme = 'light'
                }
                this.preferences = { ...DEFAULT_PREFERENCES, ...parsed }
            }
        } catch (error) {
            console.warn('Failed to load user preferences from localStorage:', error)
            this.preferences = DEFAULT_PREFERENCES
        }
    }

    private save(): void {
        if (!browser) return

        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(this.preferences))
        } catch (error) {
            console.warn('Failed to save user preferences to localStorage:', error)
        }
    }

    get<K extends keyof UserPreferences>(key: K): UserPreferences[K] {
        return this.preferences[key]
    }

    set<K extends keyof UserPreferences>(key: K, value: UserPreferences[K]): void {
        this.preferences[key] = value
        this.save()
    }

    update(updates: Partial<UserPreferences>): void {
        this.preferences = { ...this.preferences, ...updates }
        this.save()
    }

    reset(): void {
        this.preferences = DEFAULT_PREFERENCES
        if (browser) {
            try {
                localStorage.removeItem(STORAGE_KEY)
            } catch (error) {
                console.warn('Failed to remove user preferences from localStorage:', error)
            }
        }
    }

    getAll(): UserPreferences {
        return { ...this.preferences }
    }
}

export const preferencesStorage = new PreferencesStorage()
