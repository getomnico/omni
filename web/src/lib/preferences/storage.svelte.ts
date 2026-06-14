import { browser } from '$app/environment'
import {
    DEFAULT_PREFERENCES,
    STORAGE_KEY,
    type ThemePreference,
    type UserPreferences,
} from './user-preferences'

function normalizeThemePreference(theme: unknown): ThemePreference | undefined {
    if (theme === 'bright') return 'light'
    if (theme === 'light' || theme === 'dark' || theme === 'system') return theme
    return undefined
}

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
                const theme = normalizeThemePreference(parsed.theme)
                this.preferences = {
                    ...DEFAULT_PREFERENCES,
                    ...parsed,
                    theme: theme ?? DEFAULT_PREFERENCES.theme,
                }
                if (theme === 'light') this.preferences.themeColorScheme = 'light'
                if (theme === 'dark') this.preferences.themeColorScheme = 'dark'
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
