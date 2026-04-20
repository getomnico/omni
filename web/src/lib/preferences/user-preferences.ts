import type { InputMode } from '$lib/components/user-input.svelte'

export interface UserPreferences {
    inputMode: InputMode
    preferredModelId: string | null
    theme: string
    themeColorScheme: 'light' | 'dark'
}

export const DEFAULT_PREFERENCES: UserPreferences = {
    inputMode: 'chat',
    preferredModelId: null,
    theme: 'bright',
    themeColorScheme: 'light',
}

export const STORAGE_KEY = 'omni-user-preferences'
