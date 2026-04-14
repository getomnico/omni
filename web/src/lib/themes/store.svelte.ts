// web/src/lib/themes/store.svelte.ts
import { preferencesStorage } from '$lib/preferences/storage.svelte'
import { getTheme } from './registry'
import type { Theme } from './types'

class ThemeStore {
    current = $state<Theme>(getTheme(preferencesStorage.get('theme')))

    constructor() {
        // Migration: persist themeColorScheme for the flash-prevention script
        // if not stored yet (users who had a theme set before this field existed)
        if (!preferencesStorage.get('themeColorScheme')) {
            preferencesStorage.set('themeColorScheme', this.current.colorScheme)
        }
    }

    set(id: string): void {
        this.current = getTheme(id)
        preferencesStorage.set('theme', id)
        preferencesStorage.set('themeColorScheme', this.current.colorScheme)
    }
}

export const themeStore = new ThemeStore()
