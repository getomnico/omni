import type { InputMode } from '$lib/components/user-input.svelte'

export interface UserPreferences {
    inputMode: InputMode
    preferredModelId: string | null
}

export const DEFAULT_PREFERENCES: UserPreferences = {
    inputMode: 'chat',
    preferredModelId: null,
}

export const STORAGE_KEY = 'omni-user-preferences'
