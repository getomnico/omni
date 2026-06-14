// web/src/lib/themes/store.svelte.ts
import { browser } from '$app/environment'
import { preferencesStorage } from '$lib/preferences/storage.svelte'
import type { ThemePreference } from '$lib/preferences/user-preferences'
import { getTheme } from './registry'
import type { Theme } from './types'

function normalizeThemePreference(theme: string): ThemePreference {
    if (theme === 'dark' || theme === 'system') return theme
    return 'light'
}

function systemThemeId(): 'light' | 'dark' {
    if (!browser) return 'light'
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

function effectiveThemeId(preference: ThemePreference): 'light' | 'dark' {
    return preference === 'system' ? systemThemeId() : preference
}

class ThemeStore {
    preference = $state<ThemePreference>(normalizeThemePreference(preferencesStorage.get('theme')))
    current = $state<Theme>(getTheme(effectiveThemeId(this.preference)))

    constructor() {
        // Migration: persist themeColorScheme for the flash-prevention script
        // if not stored yet (users who had a theme set before this field existed)
        if (!preferencesStorage.get('themeColorScheme')) {
            preferencesStorage.set('themeColorScheme', this.current.colorScheme)
        }

        if (browser) {
            window
                .matchMedia('(prefers-color-scheme: dark)')
                .addEventListener('change', () => this.refreshCurrentTheme())
        }
    }

    set(preference: ThemePreference): void {
        this.preference = preference
        this.refreshCurrentTheme()
        preferencesStorage.update({
            theme: preference,
            themeColorScheme: this.current.colorScheme,
        })
    }

    refreshCurrentTheme(): void {
        this.current = getTheme(effectiveThemeId(this.preference))
        preferencesStorage.set('themeColorScheme', this.current.colorScheme)
    }
}

export const themeStore = new ThemeStore()
