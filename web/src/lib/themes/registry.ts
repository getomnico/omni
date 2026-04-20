import type { Theme } from './types'
import { bright } from './bright'
import { dark } from './dark'

export const themes: Theme[] = [bright, dark]

export function getTheme(id: string): Theme {
    return themes.find((t) => t.id === id) ?? bright
}
